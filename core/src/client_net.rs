//! Сетевой поток отправляющей стороны: соединение, отправка событий пачками, heartbeat, обработка кика.

use std::io::{self, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use switchdesk_proto::{coalesce, encode_frame, read_msg, write_msg, ClientMsg, HostMsg, VERSION};

use crate::platform::{self as win, Beep};
use crate::settings::Settings;
use crate::state::{self, Mode};
use crate::{engine, host};

pub enum Event {
    /// Ввод с хуков (только в режиме «удалённо»).
    Input(ClientMsg),
    /// Локальный буфер обмена изменился.
    Clip(String),
    /// Горячая клавиша: войти в «удалённо».
    Enter,
    /// Горячая клавиша: выйти (локальный режим уже включён хуком).
    Leave,
    /// Сообщение от хоста (id соединения, сообщение).
    FromHost(u64, HostMsg),
    /// Соединение закрылось.
    Closed(u64),
}

struct Conn {
    id: u64,
    stream: TcpStream,
    last_rx: Instant,
    last_ping: Instant,
    ping_seq: u32,
}

const PING_EVERY: Duration = Duration::from_secs(1);
const HOST_TIMEOUT: Duration = Duration::from_secs(5);

fn other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn connect(addr: &str, name: &str, id: u64, tx: &Sender<Event>) -> io::Result<Conn> {
    let addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| other("адрес не разрешился"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;

    write_msg(&mut stream, &ClientMsg::Hello { version: VERSION, name: name.to_owned() })?;
    match read_msg::<HostMsg, _>(&mut stream)? {
        HostMsg::Welcome { host_name, .. } => state::log(format!("Подключено: {host_name}")),
        HostMsg::Rejected { reason } => return Err(other(format!("хост отклонил: {reason}"))),
        _ => return Err(other("неожиданный ответ хоста")),
    }
    stream.set_read_timeout(None)?;

    let mut reader = stream.try_clone()?;
    let tx = tx.clone();
    thread::spawn(move || loop {
        match read_msg::<HostMsg, _>(&mut reader) {
            Ok(msg) => {
                if tx.send(Event::FromHost(id, msg)).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = tx.send(Event::Closed(id));
                return;
            }
        }
    });

    let now = Instant::now();
    Ok(Conn { id, stream, last_rx: now, last_ping: now, ping_seq: 0 })
}

/// Отправляет накопленные события одним write. `false` — запись не удалась.
fn flush(conn: &mut Option<Conn>, pending: &mut Vec<ClientMsg>) -> bool {
    if pending.is_empty() {
        return true;
    }
    let msgs = coalesce(std::mem::take(pending));
    let Some(c) = conn.as_mut() else { return true };
    let mut buf = Vec::with_capacity(msgs.len() * 16);
    for m in &msgs {
        encode_frame(m, &mut buf);
    }
    c.stream.write_all(&buf).is_ok()
}

/// Разрывает соединение и возвращает ввод на этот компьютер.
fn drop_conn(conn: &mut Option<Conn>, reason: &str, beep: Option<Beep>) {
    if let Some(c) = conn.take() {
        let _ = c.stream.shutdown(Shutdown::Both);
        state::log(format!("Отключено: {reason}"));
    }
    win::leave_remote();
    state::set_mode(Mode::Local);
    if let Some(b) = beep {
        win::beep(b);
    }
}

pub fn run(rx: Receiver<Event>, tx: Sender<Event>) {
    let mut conn: Option<Conn> = None;
    let mut next_id = 1u64;
    let mut pending: Vec<ClientMsg> = Vec::new();

    loop {
        let mut events = Vec::new();
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(e) => events.push(e),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        events.extend(rx.try_iter().take(512));

        for ev in events {
            match ev {
                Event::Input(m) => {
                    if conn.is_some() {
                        pending.push(m);
                    }
                }
                Event::Clip(t) => {
                    if conn.is_some() {
                        pending.push(ClientMsg::Clipboard(t));
                    }
                }
                Event::Enter => {
                    if conn.is_none() {
                        enter(&mut conn, &mut next_id, &mut pending, &tx);
                    }
                }
                Event::Leave => {
                    // Хук уже вернул ввод локально; здесь — вежливо закрываем сессию.
                    if conn.is_some() {
                        pending.push(ClientMsg::ReleaseAll);
                        let _ = flush(&mut conn, &mut pending);
                    }
                    drop_conn(&mut conn, "вы вернулись на этот компьютер", None);
                }
                Event::FromHost(id, msg) => {
                    let Some(c) = conn.as_mut().filter(|c| c.id == id) else { continue };
                    c.last_rx = Instant::now();
                    match msg {
                        HostMsg::Kicked { by } => {
                            pending.clear();
                            drop_conn(&mut conn, &format!("компьютер занял «{by}»"), Some(Beep::Error));
                        }
                        HostMsg::LocalActivity => {
                            state::log("За тем компьютером сейчас работают физически — ваш ввод временно игнорируется");
                            win::beep(Beep::Notice);
                        }
                        HostMsg::Clipboard(t) => win::apply_host_clipboard(&t),
                        HostMsg::Rejected { reason } => {
                            drop_conn(&mut conn, &format!("отклонено: {reason}"), Some(Beep::Error))
                        }
                        HostMsg::Welcome { .. } | HostMsg::Pong(_) => {}
                    }
                }
                Event::Closed(id) => {
                    if conn.as_ref().is_some_and(|c| c.id == id) {
                        pending.clear();
                        drop_conn(&mut conn, "соединение закрыто", Some(Beep::Error));
                    }
                }
            }
        }

        // Heartbeat и контроль живости хоста.
        if let Some(c) = conn.as_mut() {
            if c.last_ping.elapsed() >= PING_EVERY {
                c.last_ping = Instant::now();
                c.ping_seq = c.ping_seq.wrapping_add(1);
                pending.push(ClientMsg::Ping(c.ping_seq));
            }
        }
        if !flush(&mut conn, &mut pending) {
            drop_conn(&mut conn, "ошибка записи в сеть", Some(Beep::Error));
        } else if conn.as_ref().is_some_and(|c| c.last_rx.elapsed() > HOST_TIMEOUT) {
            drop_conn(&mut conn, "компьютер не отвечает", Some(Beep::Error));
        }
    }
}

/// Горячая клавиша / кнопка: подключиться к выбранному компьютеру и начать отправку ввода.
fn enter(conn: &mut Option<Conn>, next_id: &mut u64, pending: &mut Vec<ClientMsg>, tx: &Sender<Event>) {
    let settings: Settings = engine::current_settings();
    let Some(peer) = settings.target_peer().cloned() else {
        state::log("Не выбран компьютер для переключения — добавьте его в окне Switchdesk");
        win::beep(Beep::Error);
        return;
    };
    if host::net::manager().has_active() {
        state::log("Этим компьютером сейчас управляют — переключиться нельзя");
        win::beep(Beep::Error);
        return;
    }
    state::set_mode(Mode::Connecting(peer.name.clone()));
    match connect(&peer.addr, &settings.device_name, *next_id, tx) {
        Ok(c) => {
            *next_id += 1;
            *conn = Some(c);
            win::enter_remote();
            state::set_mode(Mode::Remote(peer.name.clone()));
            state::log(format!("Ввод переключён на «{}»", peer.name));
            if let Some(text) = win::initial_clipboard() {
                pending.push(ClientMsg::Clipboard(text));
            }
        }
        Err(e) => {
            state::set_mode(Mode::Local);
            state::log(format!("Не удалось подключиться к «{}» ({}): {e}", peer.name, peer.addr));
            win::beep(Beep::Error);
        }
    }
}
