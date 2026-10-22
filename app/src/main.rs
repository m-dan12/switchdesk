#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod client;
mod clip;
mod config;
mod host;
mod inject;
mod proto;
mod updater;
mod winapp;

fn main() -> eframe::Result {
    if !winapp::claim_single_instance() {
        return Ok(());
    }
    let args: Vec<String> = std::env::args().collect();
    let minimized = args.iter().any(|a| a == "--minimized");
    // служебный флаг для проверки внешнего вида: --tab host|settings
    let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned();
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
