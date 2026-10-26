//! Фоновое ядро: хост, клиентские соединения, перехват ввода, трей и обновления. Работает без окна
//! и почти не занимает память; окно — отдельный процесс (`--gui`), он открывается из трея.

use crate::capture;
use crate::client::Client;
use crate::config::Config;
use crate::host::{Host, HostShared};
use crate::ipc::{ClientView, Req, State, UpdView};
use crate::updater::{UpdState, Updater};
use crate::winapp;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

pub struct Core {
    client: Client,
    updater: Updater,
    shared: Arc<HostShared>,
    host: Mutex<Option<Host>>,
    host_error: Mutex<Option<String>>,
}

impl Core {
    fn new() -> Arc<Core> {
        let cfg = Config::load();
        Arc::new(Core {
            client: Client::new(),
            updater: Updater::new(),
            shared: HostShared::new(cfg.host_allow_input),
            host: Mutex::new(None),
            host_error: Mutex::new(None),
        })
    }

    /// Применяет настройки хоста из config.json: запускает или останавливает приём подключений.
    fn host_apply(&self) {
        let mut cfg = Config::load();
        self.shared.allow_input.store(cfg.host_allow_input, SeqCst);
        let mut host = self.host.lock().unwrap();
        if cfg.host_enabled && host.is_none() {
            match Host::start(cfg.host_port, cfg.host_token.clone(), cfg.pc_name.clone(), self.shared.clone()) {
                Ok(h) => {
                    *host = Some(h);
                    *self.host_error.lock().unwrap() = None;
                }
                Err(e) => {
                    *self.host_error.lock().unwrap() = Some(format!("Не удалось открыть порт {}: {e}", cfg.host_port));
                    cfg.host_enabled = false;
                    cfg.save();
                }
            }
        } else if !cfg.host_enabled && host.is_some() {
            *host = None; // Drop закрывает все соединения
            self.shared.clients.lock().unwrap().clear();
        }
    }

    fn state(&self) -> State {
        State {
            status: self.client.status(),
            active: capture::is_active(),
            host_running: self.host.lock().unwrap().is_some(),
            host_error: self.host_error.lock().unwrap().clone(),
            clients: self
                .shared
                .clients
                .lock()
                .unwrap()
                .iter()
                .map(|c| ClientView { name: c.name.clone(), ip: c.addr.ip().to_string() })
                .collect(),
            update: match self.updater.state() {
                UpdState::Idle => UpdView::Idle,
                UpdState::Checking => UpdView::Checking,
                UpdState::UpToDate => UpdView::UpToDate,
                UpdState::Available(r) => UpdView::Available(r.version.to_string()),
                UpdState::Downloading(r) => UpdView::Downloading(r.version.to_string()),
                UpdState::Error(e) => UpdView::Error(e),
            },
            elevated: winapp::is_elevated(),
        }
    }

    fn handle(&self, req: Req) -> State {
        match req {
            Req::State => {}
            Req::Connect { dev, me } => self.client.connect(dev, me),
            Req::Disconnect => self.client.disconnect(),
            Req::Toggle => capture::toggle(),
            Req::HostApply => self.host_apply(),
            Req::UpdateCheck => self.updater.check(),
            Req::UpdateInstall => self.updater.install(),
            Req::RestartElevated => winapp::restart_elevated(),
        }
        self.state()
    }
}

/// Запускает ядро и крутит цикл сообщений трея; не возвращается до выхода.
pub fn run(open_gui: bool) {
    let core = Core::new();
    core.host_apply();
    crate::control::listen(core.client.clone());
    {
        let core = core.clone();
        crate::ipc::serve(move |req| core.handle(req));
    }
    core.updater.check();
    {
        let updater = core.updater.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(UPDATE_EVERY);
            updater.check();
        });
    }
    let _tray = make_tray(&core);
    if open_gui {
        winapp::open_gui();
    }
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Значок в трее: левый клик открывает окно, меню — быстрые действия.
fn make_tray(core: &Arc<Core>) -> Option<tray_icon::TrayIcon> {
    let menu = Menu::new();
    let open = MenuItem::new("Открыть SwitchDesk", true, None);
    let release = MenuItem::new("Вернуть управление себе", true, None);
    let disconnect = MenuItem::new("Отключиться", true, None);
    let update = MenuItem::new("Проверить обновления", true, None);
    let quit = MenuItem::new("Выход", true, None);
    menu.append_items(&[&open, &release, &disconnect, &update, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let (open_id, release_id, disconnect_id, update_id, quit_id) =
        (open.id().clone(), release.id().clone(), disconnect.id().clone(), update.id().clone(), quit.id().clone());
    let core = core.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            winapp::open_gui();
        } else if e.id == release_id {
            if capture::is_active() {
                capture::toggle();
            }
        } else if e.id == disconnect_id {
            core.client.disconnect();
        } else if e.id == update_id {
            core.updater.check();
            winapp::open_gui();
        } else if e.id == quit_id {
            capture::set_sink(None);
            std::process::exit(0);
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            winapp::open_gui();
        }
    }));
    let icon = Icon::from_resource(1, Some((32, 32))).ok()?;
    TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("SwitchDesk").with_icon(icon).build().ok()
}
