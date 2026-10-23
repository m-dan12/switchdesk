//! Перехват мыши и клавиатуры на стороне клиента (глобальные low-level хуки).
//!
//! Пока захват включён, события не доходят до локальных программ (хук возвращает 1)
//! и уходят в канал `SINK`. Мышь передаётся относительными смещениями от центра
//! экрана: курсор при захвате замирает в центре.
//! Горячая клавиша Ctrl+Alt+F12 включает/выключает захват.

use crate::proto::Ev;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, Once};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CONNECTED: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);
static SWALLOW_F12_UP: AtomicBool = AtomicBool::new(false);
static RAW_MOVES: AtomicU64 = AtomicU64::new(0);
static KEYS_SEEN: AtomicU64 = AtomicU64::new(0);
static MOUSE_SEEN: AtomicU64 = AtomicU64::new(0);
/// Кольцевой буфер последних клавиш для диагностики (сбрасывается в hook.log при переключении).
static TRACE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static TOUCH: Mutex<Option<crate::touchpad::Touchpads>> = Mutex::new(None);
/// Почти невидимое окно на весь экран, пока идёт передача: системная прокрутка тачпада уходит в него,
/// а не в окно под замершим курсором.
static SHIELD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static SINK: Mutex<Option<Sender<Ev>>> = Mutex::new(None);
static PRESSED: Mutex<Vec<(u16, bool)>> = Mutex::new(Vec::new());

const VK_F12: u32 = 0x7B;
/// Поток с хуками и его сообщение «переставить хуки наверх цепочки».
static HOOK_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const WM_REHOOK: u32 = WM_APP + 1;

fn shield(on: bool) {
    let h = SHIELD.load(SeqCst);
    if h == 0 {
        return;
    }
    let hwnd = HWND(h as *mut _);
    unsafe {
        if on {
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let (w, hh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, hh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe extern "system" fn shield_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

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
        shield(false);
        PRESSED.lock().unwrap().clear();
    }
}

fn emit(ev: Ev) {
    if let Some(tx) = SINK.lock().unwrap().as_ref() {
        let _ = tx.send(ev);
    }
}

pub(crate) fn log(msg: &str) {
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
        shield(false);
        PRESSED.lock().unwrap().clear();
        emit(Ev::Release);
    } else if CONNECTED.load(SeqCst) {
        // Курсор остаётся там, где был: движение мыши в хуке гасится, а на хост уходят сырые дельты.
        RAW_MOVES.store(0, SeqCst);
        if let Some(t) = TOUCH.lock().unwrap().as_mut() {
            t.reset();
        }
        // Другие программы (например, радиальное меню на боковых кнопках) тоже ставят хуки и
        // переставляются наверх цепочки: без этого они забирают кнопки раньше нас.
        let tid = HOOK_TID.load(SeqCst);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_REHOOK, WPARAM(0), LPARAM(0));
            }
        }
        ACTIVE.store(true, SeqCst);
        shield(true);
        log("capture ON");
    } else {
        log("toggle ignored: not connected");
    }
}

unsafe extern "system" fn mouse_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        MOUSE_SEEN.fetch_add(1, SeqCst);
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
        KEYS_SEEN.fetch_add(1, SeqCst);
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
                if ri.header.dwType == RIM_TYPEHID.0 {
                    // Сырой отчёт тачпада: прокрутка двумя пальцами.
                    let off = hdr as usize;
                    let (size_hid, count) = (ri.data.hid.dwSizeHid as usize, ri.data.hid.dwCount as usize);
                    let data = buf.get(off + 8..off + 8 + size_hid * count).unwrap_or(&[]);
                    let evs = TOUCH.lock().unwrap().get_or_insert_with(Default::default).feed(ri.header.hDevice, size_hid, data);
                    for e in evs {
                        emit(e);
                    }
                } else if ri.header.dwType == RIM_TYPEMOUSE.0 {
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

/// Раз в секунду пишет, изменились ли счётчики хука и что сейчас в фокусе (только при SWITCHDESK_DEBUG).
fn heartbeat_thread() {
    let mut last = (0u64, 0u64, String::new());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (k, m) = (KEYS_SEEN.load(SeqCst), MOUSE_SEEN.load(SeqCst));
        let fg = crate::inject::foreground_title();
        if last.0 != k || last.1 != m || last.2 != fg {
            log(&format!("hb keys={k} mouse={m} active={} focus={fg}", ACTIVE.load(SeqCst)));
            last = (k, m, fg);
        }
    }
}

fn register_raw(hwnd: HWND) {
    let mouse = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    // Precision Touchpad (страница Digitizer, usage 5).
    let pad = RAWINPUTDEVICE { usUsagePage: 0x0D, usUsage: 5, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    unsafe {
        let _ = RegisterRawInputDevices(&[mouse, pad], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
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
        let sclass = w!("SwitchDeskShield");
        let swc = WNDCLASSW { lpfnWndProc: Some(shield_wndproc), hInstance: hinst, lpszClassName: sclass, ..Default::default() };
        RegisterClassW(&swc);
        if let Ok(sh) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            sclass,
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
        ) {
            let _ = SetLayeredWindowAttributes(sh, windows::Win32::Foundation::COLORREF(0), 1, LWA_ALPHA);
            SHIELD.store(sh.0 as isize, SeqCst);
        }
        SetTimer(Some(hwnd), 1, 250, None);
        log("raw input window ready");
    }
}

/// Запускает поток с хуками (один раз за процесс).
pub fn start() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if debug() {
            std::thread::spawn(heartbeat_thread);
        }
        std::thread::spawn(|| unsafe {
            let hmod = GetModuleHandleW(None).unwrap_or_default();
            let hinst = HINSTANCE(hmod.0);
            HOOK_TID.store(GetCurrentThreadId(), SeqCst);
            let install = || {
                (
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), Some(hinst), 0).ok(),
                    SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0).ok(),
                )
            };
            let mut hooks = install();
            create_raw_window(hinst);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.hwnd.is_invalid() && msg.message == WM_REHOOK {
                    // Новые хуки встают первыми в цепочке; старые снимаем после установки.
                    let fresh = install();
                    if let Some(h) = hooks.0.take() {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    if let Some(h) = hooks.1.take() {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    hooks = fresh;
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
    });
}
