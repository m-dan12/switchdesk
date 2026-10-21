//! Движок: запускает все части и применяет настройки.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use switchdesk_update::Release;

use crate::host::net as host_net;
use crate::settings::{self, normalize_addr, Settings};
use crate::state::{self, Listen, Mode, Snapshot, UpdateState};
use crate::{client_net, platform};

static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);
static LISTENER: Mutex<Option<host_net::Listener>> = Mutex::new(None);
static RELEASE: Mutex<Option<Release>> = Mutex::new(None);
static NO_AUTO_UPDATE: AtomicBool = AtomicBool::new(false);

const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const LISTEN_CHECK_EVERY: Duration = Duration::from_secs(10);

pub(crate) fn current_settings() -> Settings {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Адрес, на котором нужно слушать, или причина, почему нельзя.
fn resolve_listen(s: &Settings) -> Result<String, String> {
    if s.listen.trim().eq_ignore_ascii_case("auto") {
        return switchdesk_win::net::tailscale_ip()
            .map(|ip| format!("{ip}:{}", s.port))
            .ok_or_else(|| "Tailscale не найден: запустите его или укажите адрес вручную".to_string());
    }
    let l = s.listen.trim();
    let with_port = if l.contains(':') { l.to_owned() } else { format!("{l}:{}", s.port) };
    normalize_addr(&with_port).ok_or_else(|| format!("неверный адрес для приёма: «{l}»"))
}

fn exe_command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\" --minimized")
}

/// Приводит слушающий порт в соответствие с настройками.
fn sync_listener() {
    let s = current_settings();
    let mut slot = LISTENER.lock().unwrap_or_else(|e| e.into_inner());
    if !s.accept_input {
        if let Some(l) = slot.take() {
            l.stop();
        }
        state::set_listen(Listen::Off);
        return;
    }
    let want = match resolve_listen(&s) {
        Ok(a) => a,
        Err(e) => {
            if let Some(l) = slot.take() {
                l.stop();
            }
            state::set_listen(Listen::Error(e));
            return;
        }
    };
    if slot.as_ref().is_some_and(|l| l.addr == want) {
        return;
    }
    if let Some(l) = slot.take() {
        l.stop();
    }
    match host_net::start_listener(&want) {
        Ok(l) => {
            state::log(format!("Принимаю ввод на {want}"));
            state::set_listen(Listen::On(l.addr.clone()));
            *slot = Some(l);
        }
        Err(e) => state::set_listen(Listen::Error(format!("не удалось занять {want}: {e}"))),
    }
}

fn idle() -> bool {
    let s = state::snapshot();
    s.mode == Mode::Local && s.hosting.is_empty()
}

pub struct Engine;

impl Engine {
    pub fn start(settings: Settings, no_auto_update: bool) -> Engine {
        state::set_log_file(settings::config_dir().join("switchdesk.log"));
        NO_AUTO_UPDATE.store(no_auto_update, Ordering::Relaxed);
        let autostart = settings.autostart;
        *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings);
        state::log(format!("Switchdesk {} запущен", switchdesk_update::CURRENT));

        let (tx, rx) = mpsc::channel();
        platform::set_sender(tx.clone());
        thread::spawn(move || client_net::run(rx, tx));
        platform::start();

        switchdesk_win::autostart::set(autostart, &exe_command());
        sync_listener();
        // Tailscale может подняться позже запуска приложения (или сменить адрес).
        thread::spawn(|| loop {
            thread::sleep(LISTEN_CHECK_EVERY);
            sync_listener();
        });
        thread::spawn(update_loop);
        Engine
    }

    pub fn settings(&self) -> Settings {
        current_settings()
    }

    pub fn snapshot(&self) -> Snapshot {
        state::snapshot()
    }

    /// То же, что горячая клавиша.
    pub fn toggle(&self) {
        platform::toggle();
    }

    /// Применяет и сохраняет настройки.
    pub fn apply(&self, mut new: Settings) -> Result<(), String> {
        new.sanitize();
        let old = current_settings();
        if new == old {
            return Ok(());
        }
        *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(new.clone());
        if new.autostart != old.autostart && !switchdesk_win::autostart::set(new.autostart, &exe_command()) {
            state::log("Не удалось изменить автозапуск");
        }
        sync_listener();
        settings::save(&new).map_err(|e| format!("не удалось сохранить настройки: {e}"))
    }

    /// Возвращает ввод на этот компьютер и отпускает всё, что зажато от чужих сессий.
    pub fn shutdown(&self) {
        platform::leave_remote();
        host_net::manager().reset();
    }

    pub fn check_update(&self) {
        thread::spawn(check_update_now);
    }

    /// Скачивает, проверяет подпись и запускает установщик; затем программа завершается.
    pub fn install_update(&self, show_window_after: bool) {
        thread::spawn(move || install_update_now(show_window_after));
    }
}

fn check_update_now() {
    state::set_update(UpdateState::Checking);
    match switchdesk_update::check() {
        Ok(Some(r)) => {
            state::log(format!("Доступна версия {}", r.version));
            state::set_update(UpdateState::Available(r.version.to_string()));
            *RELEASE.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        }
        Ok(None) => state::set_update(UpdateState::UpToDate),
        Err(e) => {
            state::log(format!("Обновления: {e}"));
            state::set_update(UpdateState::Failed(e));
        }
    }
}

fn install_update_now(show_window_after: bool) {
    let Some(rel) = RELEASE.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    state::set_update(UpdateState::Installing);
    state::log(format!("Скачиваю версию {}…", rel.version));
    let result = switchdesk_update::download(&rel).and_then(|path| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut args = vec!["--no-auto-update".to_string()];
        if !show_window_after {
            args.push("--minimized".into());
        }
        switchdesk_update::run_installer(&path, Some((exe, args))).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            state::log("Подпись проверена, устанавливаю обновление");
            Engine.shutdown();
            std::process::exit(0);
        }
        Err(e) => {
            state::log(format!("Не удалось обновиться: {e}"));
            state::set_update(UpdateState::Failed(e));
        }
    }
}

/// Проверка при запуске и раз в несколько часов; сама ставит обновление, только когда
/// никто не пользуется вводом.
fn update_loop() {
    thread::sleep(Duration::from_secs(5));
    loop {
        check_update_now();
        let available = matches!(state::snapshot().update, UpdateState::Available(_));
        if available && current_settings().auto_update && !NO_AUTO_UPDATE.load(Ordering::Relaxed) && idle() {
            install_update_now(false);
        }
        thread::sleep(UPDATE_EVERY);
    }
}
