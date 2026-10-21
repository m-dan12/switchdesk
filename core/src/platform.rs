//! Windows-часть: глобальные хуки, Raw Input, горячая клавиша, буфер обмена.
//!
//! Принцип: в режиме «удалённо» хуки проглатывают локальный ввод и кладут события
//! в канал сетевого потока. Переключение «обратно» выполняется прямо в хуке —
//! не дожидаясь сети, поэтому из зависшей сессии всегда есть выход.
//! Заодно хуки замечают физический ввод за этим компьютером (приоритет над удалённым).

use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::thread;

use switchdesk_proto::{Button, ClientMsg, HostMsg};
use switchdesk_win::{clipboard, inject, tick_ms, window};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::AddClipboardFormatListener;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, MOUSE_VIRTUAL_DESKTOP, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DefWindowProcW, GetSystemMetrics, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_INJECTED, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, LLKHF_UP, LLMHF_INJECTED, MB_ICONEXCLAMATION, MB_ICONHAND,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_CLIPBOARDUPDATE, WM_INPUT, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::client_net::Event;
use crate::host::net as host_net;

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
pub(crate) static CLIP_LAST: Mutex<String> = Mutex::new(String::new());
/// Момент (tick_ms) последнего физического ввода на этом компьютере; 0 — не было.
static LAST_LOCAL_MS: AtomicU64 = AtomicU64::new(0);
static GRACE_MS: AtomicU64 = AtomicU64::new(1500);

fn send(ev: Event) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(ev);
    }
}

pub fn set_sender(tx: Sender<Event>) {
    let _ = TX.set(tx);
}

pub fn set_grace(ms: u64) {
    GRACE_MS.store(ms, Relaxed);
}

/// Работал ли кто-то за этим компьютером физически совсем недавно.
pub fn local_recent() -> bool {
    let grace = GRACE_MS.load(Relaxed);
    let last = LAST_LOCAL_MS.load(Relaxed);
    grace > 0 && last != 0 && tick_ms().saturating_sub(last) < grace
}

fn mark_local() {
    LAST_LOCAL_MS.store(tick_ms(), Relaxed);
}

/// true — ваш ввод сейчас уходит на другой компьютер.
pub fn is_remote() -> bool {
    REMOTE.load(Relaxed)
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
    *LAST_ABS.lock().unwrap() = None;
    MOVE_LOGGED.store(false, Relaxed);
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

/// То же, что горячая клавиша: войти в «удалённо» или вернуться.
pub fn toggle() {
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
            mark_local();
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
    if code == HC_ACTION as i32 {
        let m = &*(lparam as *const MSLLHOOKSTRUCT);
        if m.flags & LLMHF_INJECTED == 0 {
            mark_local();
            if !REMOTE.load(Relaxed) {
                return CallNextHookEx(ptr::null_mut(), code, wparam, lparam);
            }
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

/// Предыдущая позиция абсолютного устройства (тачпад, RDP, виртуальная машина).
static LAST_ABS: Mutex<Option<(i32, i32)>> = Mutex::new(None);
/// Чтобы написать в журнал о первом событии движения за сессию, а не о каждом.
static MOVE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Переводит абсолютные координаты устройства (0..65535) в смещение в пикселях.
fn absolute_delta(x: i32, y: i32, virtual_desktop: bool) -> Option<(i32, i32)> {
    let mut last = LAST_ABS.lock().unwrap();
    let prev = last.replace((x, y))?;
    let (cx, cy) = unsafe {
        if virtual_desktop {
            (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
        } else {
            (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        }
    };
    let dx = ((x - prev.0) as i64 * cx as i64 / 65535) as i32;
    let dy = ((y - prev.1) as i64 * cy as i64 / 65535) as i32;
    Some((dx, dy))
}

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
    if raw.header.dwType != RIM_TYPEMOUSE {
        return;
    }
    let m = raw.data.mouse;
    let absolute = m.usFlags & MOUSE_MOVE_ABSOLUTE != 0;
    let delta = if absolute {
        absolute_delta(m.lLastX, m.lLastY, m.usFlags & MOUSE_VIRTUAL_DESKTOP != 0)
    } else {
        Some((m.lLastX, m.lLastY))
    };
    if let Some((dx, dy)) = delta {
        if dx != 0 || dy != 0 {
            if !MOVE_LOGGED.swap(true, Relaxed) {
                crate::state::log(if absolute {
                    "Движение мыши: устройство абсолютное (тачпад/RDP/виртуалка), пересчитываю в смещения"
                } else {
                    "Движение мыши: события получены"
                });
            }
            send(Event::Input(ClientMsg::MouseMove { dx, dy }));
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
    let remote = REMOTE.load(Relaxed);
    let mgr = host_net::manager();
    let hosting = !remote && mgr.has_active();
    if !(remote || hosting) || clipboard::is_excluded() {
        return;
    }
    // Если за этим компьютером физически работают, его копирование не уходит удалённому пользователю.
    if hosting && local_recent() {
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
    if remote {
        send(Event::Clip(text));
    } else {
        mgr.send_active(&HostMsg::Clipboard(text));
    }
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
    let Some(hwnd) = window::create_message_window("SwitchdeskClipboard", Some(clip_wnd_proc)) else {
        crate::state::log("Не удалось создать окно буфера обмена — синхронизация отключена");
        return;
    };
    if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
        crate::state::log("AddClipboardFormatListener не удался — синхронизация отключена");
        return;
    }
    window::run_message_loop();
}

// ───────────────── запуск ─────────────────

/// Запускает потоки хуков и буфера обмена. Ошибки пишутся в журнал.
pub fn start() {
    thread::spawn(clipboard_thread);
    thread::spawn(input_thread);
}

fn input_thread() {
    unsafe {
        let Some(hwnd) = window::create_hidden_window("SwitchdeskInput", Some(input_wnd_proc)) else {
            crate::state::log("Не удалось создать окно для Raw Input — движение мыши не будет передаваться");
            return;
        };
        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01, // Generic Desktop
            usUsage: 0x02,     // Mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if RegisterRawInputDevices(&rid, 1, size_of::<RAWINPUTDEVICE>() as u32) == 0 {
            crate::state::log(format!(
                "RegisterRawInputDevices не удался ({}) — движение мыши не будет передаваться",
                std::io::Error::last_os_error()
            ));
        } else {
            crate::state::log("Raw Input для мыши зарегистрирован");
        }

        let hinst = GetModuleHandleW(ptr::null());
        if SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_proc), hinst, 0).is_null()
            || SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0).is_null()
        {
            crate::state::log("Не удалось установить глобальные хуки ввода — переключение не заработает");
            return;
        }
    }
    window::run_message_loop();
}
