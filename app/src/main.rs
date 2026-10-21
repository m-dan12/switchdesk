//! Switchdesk: одно приложение, которое и принимает ввод с других компьютеров,
//! и отправляет ваш ввод на выбранный компьютер.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod icon;
mod png;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod ui;

use switchdesk_update::cli;

const USAGE: &str = "\
switchdesk — переключатель клавиатуры и мыши между компьютерами

  switchdesk [--minimized]

  --minimized         запустить свёрнутым в трей
  --version           показать версию
  --check-update      проверить, есть ли новая версия
  --update            скачать и установить новую версию
  --no-auto-update    не обновляться автоматически в этом запуске";

#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let upd = cli::take_flags(&mut args);
    let minimized = args.iter().any(|a| a == "--minimized");
    args.retain(|a| a != "--minimized");
    // служебный режим для CI: снимки окна в нескольких состояниях
    let demo_dir = args.iter().position(|a| a == "--demo-screenshots").and_then(|i| args.get(i + 1).cloned());
    if demo_dir.is_some() {
        args.retain(|a| Some(a) != demo_dir.as_ref() && a != "--demo-screenshots");
    }

    if upd.is_command() {
        attach_console();
        cli::run("switchdesk", &upd);
    }
    if !args.is_empty() {
        attach_console();
        eprintln!("неизвестные аргументы: {}\n\n{USAGE}", args.join(" "));
        std::process::exit(2);
    }
    #[cfg(windows)]
    gui(minimized, upd.no_auto, demo_dir);
    #[cfg(not(windows))]
    {
        let _ = minimized;
        eprintln!("switchdesk работает только на Windows");
        std::process::exit(2);
    }
}

#[cfg(windows)]
fn gui(minimized: bool, no_auto_update: bool, demo_dir: Option<String>) {
    use switchdesk_core::{settings, state, Engine};

    if let Some(dir) = demo_dir {
        run_window(false, switchdesk_core::Engine, Some(dir.into()));
        return;
    }
    if !switchdesk_win::single::acquire() {
        switchdesk_win::single::request_show();
        return;
    }

    // Панику в любом потоке — в журнал: окна консоли нет, иначе причина пропадёт.
    std::panic::set_hook(Box::new(|info| {
        state::set_log_file(settings::config_dir().join("switchdesk.log"));
        state::log(format!("СБОЙ: {info}"));
    }));

    let settings = settings::load();
    let _ = settings::save(&settings);
    let engine = Engine::start(settings, no_auto_update);
    let _ = engine;

    std::thread::spawn(|| switchdesk_win::single::wait_show_requests(tray::show_main_window));
    tray::spawn(Engine);

    run_window(minimized, Engine, None);
    Engine.shutdown();
}

#[cfg(windows)]
fn run_window(minimized: bool, engine: switchdesk_core::Engine, demo: Option<std::path::PathBuf>) {
    use std::sync::Arc;

    use switchdesk_core::state;

    let icon = eframe::egui::IconData { rgba: icon::render(icon::IconState::Local, 64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(tray::WINDOW_TITLE)
            .with_inner_size([470.0, 720.0])
            .with_min_inner_size([420.0, 520.0])
            .with_icon(Arc::new(icon))
            .with_visible(!minimized),
        ..Default::default()
    };
    let result = eframe::run_native(
        tray::WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            ui::apply_theme(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            tray::set_context(ctx.clone());
            state::set_repaint(move || ctx.request_repaint());
            let app = match demo.clone() {
                Some(dir) => ui::App::demo(engine, dir),
                None => ui::App::new(engine),
            };
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = result {
        state::log(format!("Окно не запустилось: {e}"));
    }
}
