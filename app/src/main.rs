#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod client;
mod clip;
mod config;
mod control;
mod core;
mod host;
mod inject;
mod ipc;
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
    if args.iter().any(|a| a == "--gui") {
        // служебный флаг для проверки внешнего вида: --tab host|settings
        let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned();
        return gui(tab);
    }

    // Обычный запуск — фоновое ядро; без --minimized (ярлык, двойной клик) ещё и окно.
    let minimized = args.iter().any(|a| a == "--minimized");
    if winapp::core_running() {
        if !minimized {
            winapp::open_gui();
        }
        return Ok(());
    }
    // Если настроен запуск с правами администратора, обычный запуск передаётся задаче.
    if !winapp::is_elevated() && winapp::elevated_task_exists() && winapp::run_elevated_task() {
        if !minimized {
            for _ in 0..40 {
                if winapp::core_running() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            winapp::open_gui();
        }
        return Ok(());
    }
    if !winapp::claim_core() {
        return Ok(());
    }
    winapp::disable_throttling();
    capture::start();
    core::run(!minimized);
    Ok(())
}

fn gui(tab: Option<String>) -> eframe::Result {
    if !winapp::claim_gui() {
        return Ok(());
    }
    // Окно без ядра бесполезно: поднимаем ядро в фоне.
    if !winapp::core_running() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--minimized").spawn();
        }
        for _ in 0..50 {
            if ipc::call(&ipc::Req::State).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
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
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, tab.as_deref())))),
    )
}
