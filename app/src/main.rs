#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod client;
mod clip;
mod config;
mod control;
mod host;
mod inject;
mod proto;
mod touchpad;
mod updater;
mod winapp;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(t) = args.iter().position(|a| a == "--switch").and_then(|i| args.get(i + 1)) {
        control::send(t);
        return Ok(());
    }
    // Если настроен запуск с правами администратора, обычный запуск передаётся задаче.
    if !winapp::is_elevated() && winapp::elevated_task_exists() && winapp::run_elevated_task() {
        return Ok(());
    }
    if !winapp::claim_single_instance() {
        return Ok(());
    }
    let minimized = args.iter().any(|a| a == "--minimized");
    // служебный флаг для проверки внешнего вида: --tab host|settings
    let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned();
    winapp::disable_throttling();
    capture::start();
    // Запуск в трее: окно сразу создаётся скрытым, без мигания.
    let hidden = minimized && config::Config::load().close_to_tray;
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_position(if hidden { [-32000.0, -32000.0] } else { [100.0, 100.0] })
            .with_title(winapp::WINDOW_TITLE)
            .with_inner_size([820.0, 520.0])
            .with_min_inner_size([680.0, 440.0]),
        ..Default::default()
    };
    eframe::run_native(
        winapp::WINDOW_TITLE,
        opts,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, tab.as_deref())))),
    )
}
