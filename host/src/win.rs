//! Windows-часть хоста: SendInput, обнаружение физического ввода, буфер обмена, сеть.

use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use switchdesk_proto::{read_msg, write_msg, ClientMsg, HostMsg, MAX_CLIPBOARD_BYTES, VERSION};
use switchdesk_win::{clipboard, inject, tick_ms, window};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::AddClipboardFormatListener;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DefWindowProcW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
    LLMHF_INJECTED, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_CLIPBOARDUPDATE,
};

use crate::held::{Decision, HeldState};
use crate::session::{Backend, Manager, Peer};
use crate::{die, Config};

const MAX_CONNECTIONS: usize = 16;

/// Момент (tick_ms) последнего физического ввода на хосте; 0 — не было.
static LAST_LOCAL_MS: AtomicU64 = AtomicU64::new(0);
static GRACE_MS: AtomicU64 = AtomicU64::new(1500);
static MANAGER: OnceLock<Arc<Manager<HostBackend>>> = OnceLock::new();
/// Последний синхронизированный текст — защита от эха.
static CLIP_LAST: Mutex<String> = Mutex::new(String::new());

fn local_recent() -> bool {
    let grace = GRACE_MS.load(Ordering::Relaxed);
    let last = LAST_LOCAL_MS.load(Ordering::Relaxed);
    grace > 0 && last != 0 && tick_ms().saturating_sub(last) < grace
}

// ───────────────────────── backend ─────────────────────────

#[derive(Default)]
pub struct HostBackend {
    held: Mutex<HeldState>,
}

impl HostBackend {
    pub fn inject(&self, msg: &ClientMsg) -> Decision {
        let decision = self.held.lock().unwrap().decide(msg, local_recent());
        if decision == Decision::Inject {
            match *msg {
                ClientMsg::MouseMove { dx, dy } => inject::mouse_move(dx, dy),
                ClientMsg::MouseButton { button, down } => inject::mouse_button(button, down),
                ClientMsg::Wheel { delta, horizontal } => inject::wheel(delta, horizontal),
                ClientMsg::Key { scan, extended, up } => inject::key(scan, extended, up),
                _ => {}
            }
        }
        decision
    }

    pub fn set_clipboard(&self, text: &str) {
        if text.len() > MAX_CLIPBOARD_BYTES {
            return;
        }
        *CLIP_LAST.lock().unwrap() = text.to_owned();
        // private = true: не оседает в истории буфера (Win+V) общего компьютера
        clipboard::set_text(text, true);
    }
}

impl Backend for HostBackend {
    fn release_all(&self) {
        let (keys, buttons) = self.held.lock().unwrap().drain();
        for (scan, extended) in keys {
            inject::key(scan, extended, true);
        }
        for b in buttons {
            inject::mouse_button(b, false);
        }
    }

    fn clear_clipboard(&self) {
        CLIP_LAST.lock().unwrap().clear();
        clipboard::clear();
    }
}

// ───────────────────────── сеть ─────────────────────────

struct TcpPeer {
    stream: Mutex<TcpStream>,
}

impl Peer for TcpPeer {
    fn send(&self, msg: &HostMsg) {
        let mut s = self.stream.lock().unwrap();
        let _ = write_msg(&mut *s, msg);
    }
    fn close(&self) {
        let _ = self.stream.lock().unwrap().shutdown(Shutdown::Both);
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Имя клиента показывается в логе и другому пользователю — чистим.
fn sanitize_name(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).take(64).collect();
    if s.trim().is_empty() {
        "unknown".into()
    } else {
        s
    }
}

fn serve(stream: TcpStream, mgr: &Manager<HostBackend>, host_name: &str) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    // на рукопожатие даём 3 с
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = stream.try_clone()?;
    let peer = Arc::new(TcpPeer { stream: Mutex::new(stream) });

    let (version, name) = match read_msg::<ClientMsg, _>(&mut reader)? {
        ClientMsg::Hello { version, name } => (version, sanitize_name(&name)),
        _ => return Err(invalid("ожидался Hello")),
    };
    if version != VERSION {
        peer.send(&HostMsg::Rejected { reason: format!("версия протокола {version}, нужна {VERSION}") });
        return Ok(());
    }
    peer.send(&HostMsg::Welcome { version: VERSION, host_name: host_name.to_owned() });

    // клиент шлёт Ping раз в секунду; тишина дольше 10 с = обрыв
    reader.set_read_timeout(Some(Duration::from_secs(10)))?;
    let id = mgr.activate(name, peer.clone());
    let result = session_loop(&mut reader, mgr, &peer, id);
    mgr.release(id);
    peer.close();
    result
}

fn session_loop(
    reader: &mut TcpStream,
    mgr: &Manager<HostBackend>,
    peer: &TcpPeer,
    id: u64,
) -> io::Result<()> {
    let mut last_notice = Instant::now() - Duration::from_secs(60);
    loop {
        let msg: ClientMsg = read_msg(reader)?;
        match msg {
            ClientMsg::Ping(n) => peer.send(&HostMsg::Pong(n)),
            ClientMsg::Hello { .. } => return Err(invalid("повторный Hello")),
            ClientMsg::Clipboard(text) => {
                if mgr.run_if_active(id, |b| b.set_clipboard(&text)).is_none() {
                    return Ok(()); // вытеснены
                }
            }
            ClientMsg::ReleaseAll => {
                if mgr.run_if_active(id, |b| b.release_all()).is_none() {
                    return Ok(());
                }
            }
            input => match mgr.run_if_active(id, |b| b.inject(&input)) {
                None => return Ok(()), // вытеснены
                Some(Decision::Suppressed) => {
                    if last_notice.elapsed() >= Duration::from_secs(2) {
                        last_notice = Instant::now();
                        peer.send(&HostMsg::LocalActivity);
                    }
                }
                Some(_) => {}
            },
        }
    }
}

fn accept_loop(listener: TcpListener, mgr: Arc<Manager<HostBackend>>, host_name: String) {
    let live = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[host] accept: {e}");
                continue;
            }
        };
        if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            live.fetch_sub(1, Ordering::SeqCst);
            continue; // слишком много одновременных соединений — сбрасываем
        }
        let (mgr, live, host_name) = (mgr.clone(), live.clone(), host_name.clone());
        thread::spawn(move || {
            let addr = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            if let Err(e) = serve(stream, &mgr, &host_name) {
                eprintln!("[host] соединение {addr} завершено: {e}");
            }
            live.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

// ───────────────── физический ввод на хосте (хуки) ─────────────────

unsafe extern "system" fn kbd_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lparam as *const KBDLLHOOKSTRUCT);
        if k.flags & LLKHF_INJECTED == 0 {
            LAST_LOCAL_MS.store(tick_ms(), Ordering::Relaxed);
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lparam as *const MSLLHOOKSTRUCT);
        if m.flags & LLMHF_INJECTED == 0 {
            LAST_LOCAL_MS.store(tick_ms(), Ordering::Relaxed);
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

// ───────────────── буфер обмена хоста → клиент ─────────────────

fn on_clipboard_changed() {
    let Some(mgr) = MANAGER.get() else { return };
    if !mgr.has_active() {
        return;
    }
    // Если за хостом физически работают, его копирование не уходит удалённому пользователю.
    if local_recent() || clipboard::is_excluded() {
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
    mgr.send_active(&HostMsg::Clipboard(text));
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

/// Отдельный поток: сокетная запись не должна задерживать хуки (у них таймаут ~300 мс).
fn clipboard_thread() {
    let Some(hwnd) = window::create_message_window("SwitchdeskHostClipboard", Some(clip_wnd_proc)) else {
        eprintln!("[host] не удалось создать окно буфера обмена — синхронизация отключена");
        return;
    };
    if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
        eprintln!("[host] AddClipboardFormatListener не удался — синхронизация отключена");
        return;
    }
    window::run_message_loop();
}

// ───────────────────────── запуск ─────────────────────────

pub fn run(cfg: Config) {
    GRACE_MS.store(cfg.grace_ms, Ordering::Relaxed);
    let host_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "host".into());

    let listener = TcpListener::bind(&cfg.bind)
        .unwrap_or_else(|e| die(&format!("не удалось занять {}: {e}", cfg.bind)));
    if cfg.bind.starts_with("0.0.0.0") || cfg.bind.starts_with("[::]") {
        eprintln!(
            "[host] ВНИМАНИЕ: порт открыт на всех интерфейсах. Привяжитесь к адресу Tailscale (100.x.y.z)."
        );
    }

    let mgr = Arc::new(Manager::new(HostBackend::default()));
    let _ = MANAGER.set(mgr.clone());
    eprintln!("[host] {host_name}: слушаю {}", cfg.bind);

    {
        let mgr = mgr.clone();
        thread::spawn(move || accept_loop(listener, mgr, host_name));
    }
    thread::spawn(clipboard_thread);

    // Главный поток: хуки физического ввода + цикл сообщений.
    unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_hook), hinst, 0);
        let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0);
        if k.is_null() || m.is_null() {
            eprintln!("[host] не удалось поставить хуки: приоритет физического ввода отключён");
        }
    }
    window::run_message_loop();
}
