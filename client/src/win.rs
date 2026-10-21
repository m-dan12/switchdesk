//! Windows-часть клиента: глобальные хуки, Raw Input, горячая клавиша, буфер обмена.
//!
//! Принцип: в режиме «удалённо» хуки проглатывают локальный ввод и кладут события
//! в канал сетевого потока. Переключение «обратно» выполняется прямо в хуке —
//! не дожидаясь сети, поэтому из зависшей сессии всегда есть выход.

use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

use switchdesk_proto::{Button, ClientMsg};
use switchdesk_win::{clipboard, inject, window};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::AddClipboardFormatListener;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DefWindowProcW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, LLMHF_INJECTED, MB_ICONEXCLAMATION, MB_ICONHAND,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_CLIPBOARDUPDATE, WM_INPUT, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::net::{self, Event};
use crate::Config;

// Виртуальные коды (в LL-хуке модификаторы приходят уже разделёнными на левые/правые).
const VK_F12: u32 = 0x7B;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

const MOD_CTRL: u32 = 0b0000_0011;
const MOD_ALT: u32 = 0b0000_1100;

/// (бит, виртуальный код). Порядок — порядок отпускания при входе в «удалённо»:
/// Alt при ещё зажатом Ctrl не активирует меню окна, Ctrl отпускаем последним.
const MOD_TABLE: [(u32, u32); 8] = [
    (0b0000_0100, VK_LMENU),
    (0b0000_1000, VK_RMENU),
    (0b0001_0000, VK_LSHIFT),
    (0b0010_0000, VK_RSHIFT),
    (0b0100_0000, VK_LWIN),
    (0b1000_0000, VK_RWIN),
    (0b0000_0001, VK_LCONTROL),
    (0b0000_0010, VK_RCONTROL),
];

/// true — ввод уходит на хост.
static REMOTE: AtomicBool = AtomicBool::new(false);
/// Физически зажатые модификаторы (биты из MOD_TABLE).
static MODS: AtomicU32 = AtomicU32::new(0);
/// Клавиша горячей клавиши, отпускание которой нужно проглотить (заодно — защита от автоповтора).
static SUPPRESS_UP_VK: AtomicU32 = AtomicU32::new(0);
static TX: OnceLock<Sender<Event>> = OnceLock::new();
/// Последний синхронизированный текст — защита от эха.
static CLIP_LAST: Mutex<String> = Mutex::new(String::new());

fn send(ev: Event) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(ev);
    }
}

// ───────────────── вызывается сетевым потоком ─────────────────

/// Входит в режим «удалённо».
pub fn enter_remote() {
    // Локальная ОС не увидит отпускания этих клавиш (мы их проглотим) — отпускаем сами.
    let mods = MODS.load(Relaxed);
    for (bit, vk) in MOD_TABLE {
        if mods & bit != 0 {
            inject::key_up_vk(vk as u16);
        }
    }
    REMOTE.store(true, Relaxed);
}

pub fn leave_remote() {
    REMOTE.store(false, Relaxed);
}

/// Текущий текст буфера для отправки на хост при входе в сессию.
pub fn initial_clipboard() -> Option<String> {
    if clipboard::is_excluded() {
        return None;
    }
    let text = clipboard::get_text()?;
    *CLIP_LAST.lock().unwrap() = text.clone();
    Some(text)
}

pub fn apply_host_clipboard(text: &str) {
    *CLIP_LAST.lock().unwrap() = text.to_owned();
    clipboard::set_text(text, false);
}

pub enum Beep {
    Error,
    Notice,
}

pub fn beep(kind: Beep) {
    let t = match kind {
        Beep::Error => MB_ICONHAND,
        Beep::Notice => MB_ICONEXCLAMATION,
    };
    unsafe {
        MessageBeep(t);
    }
}

// ───────────────── хуки ─────────────────

fn update_mods(vk: u32, down: bool) {
    if let Some((bit, _)) = MOD_TABLE.iter().find(|(_, v)| *v == vk) {
        if down {
            MODS.fetch_or(*bit, Relaxed);
        } else {
            MODS.fetch_and(!*bit, Relaxed);
        }
    }
}

fn toggle() {
    // Выход из «удалённо» делаем здесь же и сразу: он не зависит от сети.
    if REMOTE.swap(false, Relaxed) {
        send(Event::Leave);
    } else {
        send(Event::Enter);
    }
}

unsafe extern "system" fn kbd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lparam as *const KBDLLHOOKSTRUCT);
        // Собственные синтетические события (сброс модификаторов) пропускаем как есть.
        if k.flags & LLKHF_INJECTED == 0 {
            let up = k.flags & LLKHF_UP != 0;
            update_mods(k.vkCode, !up);

            if k.vkCode == VK_F12 {
                if !up {
                    if SUPPRESS_UP_VK.load(Relaxed) == VK_F12 {
                        return 1; // автоповтор зажатой горячей клавиши
                    }
                    let m = MODS.load(Relaxed);
                    if m & MOD_CTRL != 0 && m & MOD_ALT != 0 {
                        SUPPRESS_UP_VK.store(VK_F12, Relaxed);
                        toggle();
                        return 1;
                    }
                } else if SUPPRESS_UP_VK.load(Relaxed) == VK_F12 {
                    SUPPRESS_UP_VK.store(0, Relaxed);
                    return 1;
                }
            }

            if REMOTE.load(Relaxed) {
                send(Event::Input(ClientMsg::Key {
                    scan: k.scanCode as u16,
                    extended: k.flags & LLKHF_EXTENDED != 0,
                    up,
                }));
                return 1;
            }
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && REMOTE.load(Relaxed) {
        let m = &*(lparam as *const MSLLHOOKSTRUCT);
        if m.flags & LLMHF_INJECTED == 0 {
            let hi = ((m.mouseData >> 16) & 0xFFFF) as u16;
            let button = |button, down| Some(ClientMsg::MouseButton { button, down });
            let msg = match wparam as u32 {
                WM_LBUTTONDOWN => button(Button::Left, true),
                WM_LBUTTONUP => button(Button::Left, false),
                WM_RBUTTONDOWN => button(Button::Right, true),
                WM_RBUTTONUP => button(Button::Right, false),
                WM_MBUTTONDOWN => button(Button::Middle, true),
                WM_MBUTTONUP => button(Button::Middle, false),
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let b = if hi == 1 { Button::X1 } else { Button::X2 };
                    button(b, wparam as u32 == WM_XBUTTONDOWN)
                }
                WM_MOUSEWHEEL => Some(ClientMsg::Wheel { delta: hi as i16 as i32, horizontal: false }),
                WM_MOUSEHWHEEL => Some(ClientMsg::Wheel { delta: hi as i16 as i32, horizontal: true }),
                // Движение: LL-хук даёт абсолютные координаты, упирающиеся в край экрана.
                // Настоящие дельты берём из Raw Input (см. handle_raw_input).
                _ => None,
            };
            if let Some(msg) = msg {
                send(Event::Input(msg));
            }
            return 1; // в режиме «удалённо» локально мышь не двигается
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

// ───────────────── Raw Input ─────────────────

unsafe fn handle_raw_input(lparam: LPARAM) {
    if !REMOTE.load(Relaxed) {
        return;
    }
    let mut buf = [0u64; 16]; // 128 байт с выравниванием под RAWINPUT
    let mut size = size_of::<[u64; 16]>() as u32;
    let n = GetRawInputData(
        lparam as HRAWINPUT,
        RID_INPUT,
        buf.as_mut_ptr().cast(),
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    );
    if n == 0 || n == u32::MAX {
        return;
    }
    let raw = &*(buf.as_ptr() as *const RAWINPUT);
    if raw.header.dwType == RIM_TYPEMOUSE {
        let m = raw.data.mouse;
        // Абсолютные устройства (тачпады, RDP, виртуалки) в MVP не поддерживаются.
        if m.usFlags & MOUSE_MOVE_ABSOLUTE == 0 && (m.lLastX != 0 || m.lLastY != 0) {
            send(Event::Input(ClientMsg::MouseMove { dx: m.lLastX, dy: m.lLastY }));
        }
    }
}

unsafe extern "system" fn input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        handle_raw_input(lparam);
        // для WM_INPUT нужно вызвать DefWindowProc — система освобождает данные
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ───────────────── буфер обмена ─────────────────

fn on_clipboard_changed() {
    if !REMOTE.load(Relaxed) || clipboard::is_excluded() {
        return;
    }
    let Some(text) = clipboard::get_text() else { return };
    {
        let mut last = CLIP_LAST.lock().unwrap();
        if *last == text {
            return; // эхо нашей же записи
        }
        *last = text.clone();
    }
    send(Event::Clip(text));
}

unsafe extern "system" fn clip_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        on_clipboard_changed();
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Отдельный поток, чтобы чтение буфера (повторы при занятости) не задерживало хуки.
fn clipboard_thread() {
    let Some(hwnd) = window::create_message_window("SwitchdeskClientClipboard", Some(clip_wnd_proc)) else {
        eprintln!("[client] не удалось создать окно буфера обмена — синхронизация отключена");
        return;
    };
    if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
        eprintln!("[client] AddClipboardFormatListener не удался — синхронизация отключена");
        return;
    }
    window::run_message_loop();
}

// ───────────────── запуск ─────────────────

pub fn run(cfg: Config) {
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(tx.clone());
    thread::spawn(move || net::run(rx, tx, cfg));
    thread::spawn(clipboard_thread);

    unsafe {
        let Some(hwnd) = window::create_message_window("SwitchdeskClientInput", Some(input_wnd_proc))
        else {
            crate::die("не удалось создать окно для Raw Input");
        };
        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01, // Generic Desktop
            usUsage: 0x02,     // Mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if RegisterRawInputDevices(&rid, 1, size_of::<RAWINPUTDEVICE>() as u32) == 0 {
            crate::die("RegisterRawInputDevices не удался");
        }

        let hinst = GetModuleHandleW(ptr::null());
        if SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_proc), hinst, 0).is_null()
            || SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0).is_null()
        {
            crate::die("не удалось установить глобальные хуки ввода");
        }
    }

    eprintln!("[client] готов. Ctrl+Alt+F12 — переключить ввод на хост и обратно.");
    window::run_message_loop();
}
