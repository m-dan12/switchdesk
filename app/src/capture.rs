//! Перехват мыши и клавиатуры на стороне клиента (глобальные low-level хуки).
//!
//! Пока захват включён, события не доходят до локальных программ (хук возвращает 1)
//! и уходят в канал `SINK`. Мышь передаётся относительными смещениями от центра
//! экрана: курсор при захвате замирает в центре.
//! Горячая клавиша Ctrl+Alt+F12 включает/выключает захват.

use crate::proto::Ev;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, Once};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CONNECTED: AtomicBool = AtomicBool::new(false);
static CX: AtomicI32 = AtomicI32::new(0);
static CY: AtomicI32 = AtomicI32::new(0);
static CTRL: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);
static SWALLOW_F12_UP: AtomicBool = AtomicBool::new(false);
static RAW_MOVES: AtomicU64 = AtomicU64::new(0);
/// Кольцевой буфер последних клавиш для диагностики (сбрасывается в hook.log при переключении).
static TRACE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SINK: Mutex<Option<Sender<Ev>>> = Mutex::new(None);
static PRESSED: Mutex<Vec<(u16, bool)>> = Mutex::new(Vec::new());

const VK_F12: u32 = 0x7B;

pub fn is_active() -> bool {
    ACTIVE.load(SeqCst)
}

/// Подключить/отключить получателя событий. Без получателя захват невозможен.
pub fn set_sink(s: Option<Sender<Ev>>) {
    let connected = s.is_some();
    *SINK.lock().unwrap() = s;
    CONNECTED.store(connected, SeqCst);
    if !connected {
        ACTIVE.store(false, SeqCst);
        PRESSED.lock().unwrap().clear();
    }
}

fn emit(ev: Ev) {
    if let Some(tx) = SINK.lock().unwrap().as_ref() {
        let _ = tx.send(ev);
    }
}

/// Диагностический лог рядом с конфигом (hook.log).
pub fn host_log(msg: &str) {
    if debug() {
        log(msg)
    }
}

fn log(msg: &str) {
    use std::io::Write;
    if let Some(base) = std::env::var_os("APPDATA") {
        let p = std::path::Path::new(&base).join("SwitchDesk").join("hook.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// Диагностика (коды клавиш) включается только переменной окружения SWITCHDESK_DEBUG=1.
fn debug() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var_os("SWITCHDESK_DEBUG").is_some())
}

fn trace(k: &KBDLLHOOKSTRUCT, down: bool, action: &str) {
    if !debug() {
        return;
    }
    let mut t = TRACE.lock().unwrap();
    if t.len() >= 60 {
        t.remove(0);
    }
    t.push(format!(
        "  key vk={:#04x} sc={:#04x} ext={} {} active={} -> {}",
        k.vkCode,
        k.scanCode,
        k.flags.0 & 1,
        if down { "down" } else { "up" },
        ACTIVE.load(SeqCst),
        action
    ));
}

fn dump_trace() {
    let lines: Vec<String> = std::mem::take(&mut *TRACE.lock().unwrap());
    for l in lines {
        log(&l);
    }
}

pub fn toggle() {
    dump_trace();
    if ACTIVE.load(SeqCst) {
        log(&format!("capture OFF, raw moves received: {}", RAW_MOVES.swap(0, SeqCst)));
        ACTIVE.store(false, SeqCst);
        PRESSED.lock().unwrap().clear();
        emit(Ev::Release);
    } else if CONNECTED.load(SeqCst) {
        unsafe {
            let _ = SetCursorPos(GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_ok() {
                CX.store(p.x, SeqCst);
                CY.store(p.y, SeqCst);
            }
        }
        RAW_MOVES.store(0, SeqCst);
        ACTIVE.store(true, SeqCst);
        log("capture ON");
    } else {
        log("toggle ignored: not connected");
    }
}

unsafe extern "system" fn mouse_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 || !ACTIVE.load(SeqCst) {
            return CallNextHookEx(None, code, w, l);
        }
        let m = &*(l.0 as *const MSLLHOOKSTRUCT);
        if m.flags & 1 != 0 {
            // LLMHF_INJECTED: чужая инъекция, не трогаем
            return CallNextHookEx(None, code, w, l);
        }
        let hi = (m.mouseData >> 16) as u16;
        match w.0 as u32 {
            // Движение берём из Raw Input (см. raw_wndproc): там нет ускорения указателя.
            WM_MOUSEMOVE => {}
            WM_LBUTTONDOWN => emit(Ev::Btn { b: 0, down: true }),
            WM_LBUTTONUP => emit(Ev::Btn { b: 0, down: false }),
            WM_RBUTTONDOWN => emit(Ev::Btn { b: 1, down: true }),
            WM_RBUTTONUP => emit(Ev::Btn { b: 1, down: false }),
            WM_MBUTTONDOWN => emit(Ev::Btn { b: 2, down: true }),
            WM_MBUTTONUP => emit(Ev::Btn { b: 2, down: false }),
            WM_XBUTTONDOWN => emit(Ev::Btn { b: 2 + hi as u8, down: true }),
            WM_XBUTTONUP => emit(Ev::Btn { b: 2 + hi as u8, down: false }),
            WM_MOUSEWHEEL => emit(Ev::Wheel { v: hi as i16 as i32, h: false }),
            WM_MOUSEHWHEEL => emit(Ev::Wheel { v: hi as i16 as i32, h: true }),
            _ => {}
        }
        LRESULT(1)
    }
}

unsafe extern "system" fn key_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, w, l);
        }
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        if k.flags.0 & 0x10 != 0 {
            // LLKHF_INJECTED
            return CallNextHookEx(None, code, w, l);
        }
        let msg = w.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        match k.vkCode {
            0xA2 | 0xA3 | 0x11 => CTRL.store(down, SeqCst),
            0xA4 | 0xA5 | 0x12 => ALT.store(down, SeqCst),
            _ => {}
        }
        if k.vkCode == VK_F12 {
            if down && CTRL.load(SeqCst) && ALT.load(SeqCst) {
                // Автоповтор F12 при удержании не должен переключать захват снова.
                if !SWALLOW_F12_UP.swap(true, SeqCst) {
                    trace(k, down, "HOTKEY toggle");
                    toggle();
                }
                return LRESULT(1);
            }
            if down && SWALLOW_F12_UP.load(SeqCst) {
                return LRESULT(1);
            }
            if up && SWALLOW_F12_UP.swap(false, SeqCst) {
                return LRESULT(1);
            }
        }
        if !ACTIVE.load(SeqCst) {
            trace(k, down, "pass (inactive)");
            return CallNextHookEx(None, code, w, l);
        }
        let key = (k.scanCode as u16, k.flags.0 & 1 != 0);
        if down {
            let mut p = PRESSED.lock().unwrap();
            if !p.contains(&key) {
                p.push(key);
            }
            drop(p);
            trace(k, down, "sent+swallowed");
            emit(Ev::Key { sc: key.0, ext: key.1, down: true });
            return LRESULT(1);
        }
        if up {
            let mut p = PRESSED.lock().unwrap();
            if let Some(i) = p.iter().position(|x| *x == key) {
                p.remove(i);
                drop(p);
                trace(k, down, "sent+swallowed");
                emit(Ev::Key { sc: key.0, ext: key.1, down: false });
                return LRESULT(1);
            }
            trace(k, down, "pass (up of key not sent)");
            // Клавиша была нажата до захвата (например Ctrl+Alt хоткея): отпускаем локально.
        }
        CallNextHookEx(None, code, w, l)
    }
}

/// Окно только для приёма WM_INPUT: сырые смещения мыши без ускорения Windows.
unsafe extern "system" fn raw_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_INPUT && ACTIVE.load(SeqCst) {
            let hdr = std::mem::size_of::<RAWINPUTHEADER>() as u32;
            let h = HRAWINPUT(l.0 as _);
            let mut size = 0u32;
            GetRawInputData(h, RID_INPUT, None, &mut size, hdr);
            let mut buf = vec![0u8; size as usize];
            let got = GetRawInputData(h, RID_INPUT, Some(buf.as_mut_ptr() as *mut _), &mut size, hdr);
            if got == size && size as usize >= std::mem::size_of::<RAWINPUTHEADER>() {
                let ri = std::ptr::read_unaligned(buf.as_ptr() as *const RAWINPUT);
                if ri.header.dwType == RIM_TYPEMOUSE.0 {
                    let m = ri.data.mouse;
                    // Абсолютные устройства (RDP, планшеты) пропускаем: у них нет смещений.
                    if m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0 && (m.lLastX != 0 || m.lLastY != 0) {
                        RAW_MOVES.fetch_add(1, SeqCst);
                        emit(Ev::Move { dx: m.lLastX, dy: m.lLastY });
                    }
                }
            }
        } else if msg == WM_TIMER {
            // winit перерегистрирует Raw Input на себя при смене фокуса: возвращаем своё.
            register_raw(hwnd);
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

fn register_raw(hwnd: HWND) {
    let dev = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    unsafe {
        let _ = RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

unsafe fn create_raw_window(hinst: HINSTANCE) {
    unsafe {
        let class = w!("SwitchDeskRaw");
        let wc = WNDCLASSW { lpfnWndProc: Some(raw_wndproc), hInstance: hinst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        // Обычное (скрытое) окно, а не message-only: надёжнее для WM_INPUT с INPUTSINK.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            log("raw window creation failed");
            return;
        };
        register_raw(hwnd);
        SetTimer(Some(hwnd), 1, 250, None);
        log("raw input window ready");
    }
}

/// Запускает поток с хуками (один раз за процесс).
pub fn start() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(|| unsafe {
            let hmod = GetModuleHandleW(None).unwrap_or_default();
            let hinst = HINSTANCE(hmod.0);
            let _kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), Some(hinst), 0);
            let _ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0);
            create_raw_window(hinst);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
    });
}
