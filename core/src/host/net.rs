//! Принимающая сторона: внедрение ввода, сеть, приём подключений.

use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use switchdesk_proto::{read_msg, write_msg, Button, ClientMsg, HostMsg, MAX_CLIPBOARD_BYTES, VERSION};
use switchdesk_win::{clipboard, inject};

use super::session::{Backend, Manager, Peer, Session};
use crate::{engine, platform, state};

const MAX_CONNECTIONS: usize = 16;

static MANAGER: OnceLock<Arc<Manager<HostBackend>>> = OnceLock::new();

pub fn manager() -> &'static Arc<Manager<HostBackend>> {
    MANAGER.get_or_init(|| Arc::new(Manager::new(HostBackend)))
}

// ───────────────────────── backend ─────────────────────────

pub struct HostBackend;

impl HostBackend {
    pub fn set_clipboard(&self, text: &str) {
        if text.len() > MAX_CLIPBOARD_BYTES {
            return;
        }
        *platform::CLIP_LAST.lock().unwrap() = text.to_owned();
        // private = true: не оседает в истории буфера (Win+V)
        clipboard::set_text(text, true);
    }
}

impl Backend for HostBackend {
    fn apply(&self, msg: &ClientMsg) {
        match *msg {
            ClientMsg::MouseMove { dx, dy } => inject::mouse_move(dx, dy),
            ClientMsg::MouseButton { button, down } => inject::mouse_button(button, down),
            ClientMsg::Wheel { delta, horizontal } => inject::wheel(delta, horizontal),
            ClientMsg::Key { scan, extended, up } => inject::key(scan, extended, up),
            _ => {}
        }
    }

    fn release(&self, keys: &[(u16, bool)], buttons: &[Button]) {
        for &(scan, extended) in keys {
            inject::key(scan, extended, true);
        }
        for &b in buttons {
            inject::mouse_button(b, false);
        }
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

/// Имя клиента показывается в журнале и другому пользователю — чистим.
fn sanitize_name(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).take(64).collect();
    if s.trim().is_empty() {
        "unknown".into()
    } else {
        s
    }
}

fn serve(stream: TcpStream, mgr: &Manager<HostBackend>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
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
    peer.send(&HostMsg::Welcome { version: VERSION, host_name: engine::current_settings().device_name });

    // клиент шлёт Ping раз в секунду; тишина дольше 10 с = обрыв
    reader.set_read_timeout(Some(Duration::from_secs(10)))?;
    let session = mgr.add(name, peer.clone());
    state::log(format!("«{}» подключился", session.name));
    state::set_hosting(mgr.names());
    let result = session_loop(&mut reader, mgr, &peer, &session);
    mgr.remove(&session);
    state::log(format!("«{}» отключился", session.name));
    state::set_hosting(mgr.names());
    peer.close();
    result
}

fn session_loop(reader: &mut TcpStream, mgr: &Manager<HostBackend>, peer: &TcpPeer, s: &Session) -> io::Result<()> {
    loop {
        match read_msg::<ClientMsg, _>(reader)? {
            ClientMsg::Ping(n) => peer.send(&HostMsg::Pong(n)),
            ClientMsg::Hello { .. } => return Err(invalid("повторный Hello")),
            ClientMsg::Clipboard(text) => {
                mgr.backend().set_clipboard(&text);
                // остальные подключённые тоже должны получить этот текст
                mgr.send_to_others(Some(s.id), &HostMsg::Clipboard(text));
            }
            ClientMsg::ReleaseAll => mgr.release_held(s),
            input => mgr.input(s, &input),
        }
    }
}

// ───────────────────────── приём подключений ─────────────────────────

pub struct Listener {
    stop: Arc<AtomicBool>,
    pub addr: String,
}

impl Listener {
    /// Перестаёт принимать новые подключения; уже идущая сессия не прерывается.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start_listener(bind: &str) -> io::Result<Listener> {
    let listener = TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    thread::spawn(move || accept_loop(listener, flag));
    Ok(Listener { stop, addr: bind.to_owned() })
}

fn accept_loop(listener: TcpListener, stop: Arc<AtomicBool>) {
    let live = Arc::new(AtomicUsize::new(0));
    let mgr = manager().clone();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, addr)) => {
                if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue; // слишком много одновременных соединений — сбрасываем
                }
                let (mgr, live) = (mgr.clone(), live.clone());
                thread::spawn(move || {
                    if let Err(e) = serve(stream, &mgr) {
                        state::log(format!("Соединение с {addr} закрыто: {e}"));
                    }
                    live.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                state::log(format!("Ошибка приёма подключения: {e}"));
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}
