//! Иконка в трее: открыть окно, переключить ввод, выйти.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;

use switchdesk_core::state::{self, Mode};
use switchdesk_core::Engine;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT,
};

use crate::icon::{self, IconState};

pub const WINDOW_TITLE: &str = "Switchdesk";

static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();
static QUIT: AtomicBool = AtomicBool::new(false);

pub fn set_context(ctx: eframe::egui::Context) {
    let _ = CTX.set(ctx);
}

/// Показывает окно (из трея или по запросу второго экземпляра).
pub fn show_main_window() {
    if let Some(ctx) = CTX.get() {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
    switchdesk_win::window::show_window_by_title(WINDOW_TITLE);
}

fn make_icon(state: IconState) -> Icon {
    Icon::from_rgba(icon::render(state, 32), 32, 32).expect("размер иконки совпадает с данными")
}

fn icon_state(m: &Mode, hosting: bool) -> IconState {
    match (m, hosting) {
        (Mode::Remote(_), _) => IconState::Remote,
        (_, true) => IconState::Hosting,
        _ => IconState::Local,
    }
}

pub fn spawn(engine: Engine) {
    thread::spawn(move || run(engine));
}

fn run(engine: Engine) {
    let open = MenuItem::new("Открыть Switchdesk", true, None);
    let toggle = MenuItem::new("Переключить ввод", true, None);
    let quit = MenuItem::new("Выйти", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[&open, &toggle, &PredefinedMenuItem::separator(), &quit]);

    let tray: TrayIcon = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Switchdesk")
        .with_icon(make_icon(IconState::Local))
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            state::log(format!("Не удалось создать иконку в трее: {e}"));
            return;
        }
    };

    let (open_id, toggle_id, quit_id) = (open.id().clone(), toggle.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        if ev.id == open_id {
            show_main_window();
        } else if ev.id == toggle_id {
            Engine.toggle();
        } else if ev.id == quit_id {
            QUIT.store(true, Ordering::Relaxed);
        }
    }));
    TrayIconEvent::set_event_handler(Some(|ev: TrayIconEvent| match ev {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show_main_window(),
        _ => {}
    }));

    let mut shown_gen = u64::MAX;
    let mut shown_icon: Option<IconState> = None;
    while !QUIT.load(Ordering::Relaxed) {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let gen = state::generation();
        if gen != shown_gen {
            shown_gen = gen;
            let snap = state::snapshot();
            let s = icon_state(&snap.mode, !snap.hosting.is_empty());
            if shown_icon != Some(s) {
                shown_icon = Some(s);
                let _ = tray.set_icon(Some(make_icon(s)));
            }
            let target = engine.settings().target_peer().map(|p| p.name.clone());
            let (label, enabled, tip) = match (&snap.mode, &snap.hosting, &target) {
                (Mode::Remote(p), _, _) => ("Вернуться на этот компьютер".to_string(), true, format!("Switchdesk — ввод на «{p}»")),
                (Mode::Connecting(p), _, _) => ("Подключаюсь…".to_string(), false, format!("Switchdesk — подключаюсь к «{p}»")),
                (_, who, Some(t)) => (
                    format!("Переключить на «{t}»"),
                    true,
                    if who.is_empty() { "Switchdesk — ввод на этом компьютере".to_string() } else { format!("Switchdesk — подключено: {}", who.join(", ")) },
                ),
                (_, _, None) => ("Нет компьютера для переключения".to_string(), false, "Switchdesk".to_string()),
            };
            toggle.set_text(label);
            toggle.set_enabled(enabled);
            let _ = tray.set_tooltip(Some(tip));
        }

        // Ждём сообщений окна; раз в 300 мс заглядываем в состояние.
        unsafe {
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 300, QS_ALLINPUT);
        }
    }
    drop(tray);
    engine.shutdown();
    std::process::exit(0);
}
