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
    if !winapp::claim_single_instance() {
        return Ok(());
    }
    let minimized = args.iter().any(|a| a == "--minimized");
    // служебный флаг для проверки внешнего вида: --tab host|settings
    let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned();
    winapp::disable_throttling();
    capture::start();
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(winapp::WINDOW_TITLE)
            .with_inner_size([820.0, 520.0])
            .with_min_inner_size([680.0, 440.0]),
        ..Default::default()
    };
    eframe::run_native(
        winapp::WINDOW_TITLE,
        opts,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, minimized, tab.as_deref())))),
    )
}
