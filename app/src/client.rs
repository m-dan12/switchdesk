//! Клиентская сторона: соединения с устройствами. Подключение к устройству остаётся открытым, когда
//! вы переключаетесь на другое или возвращаете ввод себе, поэтому возврат на него мгновенный.
//! Ввод и буфер обмена идут только на активное устройство.

use crate::capture;
use crate::clip::ClipSync;
use crate::config::{Device, DEFAULT_PORT};
use crate::proto::{self, Ev, Msg, Tx};
use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Как часто пустой пакет проверяет, что фоновое соединение живо.
const PING_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Status {
    #[default]
    Idle,
    Connecting,
    Connected(String),
    Error(String),
}

struct Conn {
    id: u64,
    tx: Tx,
    ev: Sender<Ev>,
    host_name: String,
    clip: Arc<ClipSync>,
}

struct Inner {
    pool: HashMap<String, Conn>,
    /// Имя устройства, на которое сейчас направлены ввод и буфер обмена.
    active: Option<String>,
    status: Status,
    /// Номер последнего запроса: устаревшие потоки не должны менять активное устройство.
    epoch: u64,
    next_id: u64,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Mutex<Inner>>,
}

impl Client {
    pub fn new() -> Client {
        Client {
            inner: Arc::new(Mutex::new(Inner {
                pool: HashMap::new(),
                active: None,
                status: Status::Idle,
                epoch: 0,
                next_id: 1,
            })),
        }
    }

    pub fn status(&self) -> Status {
        self.inner.lock().unwrap().status.clone()
    }

    /// Закрывает активное соединение совсем (фоновые остаются).
    pub fn disconnect(&self) {
        let mut g = self.inner.lock().unwrap();
        g.epoch += 1;
        if let Some(name) = g.active.take() {
            if let Some(c) = g.pool.remove(&name) {
                c.tx.close();
            }
        }
        capture::set_sink(None);
        g.status = Status::Idle;
    }

    /// Делает устройство активным: готовое соединение подхватывается сразу, иначе открывается новое.
    /// Соединение с прежним устройством не закрывается.
    pub fn connect(&self, dev: Device, my_name: String) {
        let my_epoch = {
            let mut g = self.inner.lock().unwrap();
            g.epoch += 1;
            if let Some(c) = g.pool.get(&dev.name) {
                // Пустой пакет: если соединение умерло, отправка не удастся.
                if c.tx.send(&Msg::Events { e: Vec::new() }).is_ok() && c.tx.send(&Msg::Claim).is_ok() {
                    let (ev, host) = (c.ev.clone(), c.host_name.clone());
                    c.clip.resend();
                    g.active = Some(dev.name.clone());
                    g.status = Status::Connected(host);
                    capture::set_sink(Some(ev));
                    return;
                }
                if let Some(c) = g.pool.remove(&dev.name) {
                    c.tx.close();
                }
            }
            g.active = None;
            capture::set_sink(None);
            g.status = Status::Connecting;
            g.epoch
        };
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            let fail = |msg: String| {
                let mut g = inner.lock().unwrap();
                if g.epoch == my_epoch {
                    g.status = Status::Error(msg);
                }
            };
            let stream = match open(&dev.addr) {
                Ok(s) => s,
                Err(e) => return fail(format!("не удалось подключиться: {e}")),
            };
            let (tx, mut rx, host_name) = match proto::client_handshake(stream, dev.token.trim(), &my_name) {
                Ok(v) => v,
                Err(e) => return fail(e.to_string()),
            };

            // События ввода -> хост (с объединением движений мыши); в простое проверка связи.
            let (ev_tx, ev_rx) = mpsc::channel::<Ev>();
            let sender_tx = tx.clone();
            std::thread::spawn(move || loop {
                let batch = match ev_rx.recv_timeout(PING_EVERY) {
                    Ok(first) => {
                        let mut batch = vec![first];
                        while batch.len() < 512 {
                            match ev_rx.try_recv() {
                                Ok(e) => batch.push(e),
                                Err(_) => break,
                            }
                        }
                        merge(batch)
                    }
                    Err(RecvTimeoutError::Timeout) => Vec::new(),
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if sender_tx.send(&Msg::Events { e: batch }).is_err() {
                    break;
                }
            });

            // Буфер обмена: только для активного устройства.
            let name = dev.name.clone();
            let (clip_inner, clip_tx, clip_name) = (inner.clone(), tx.clone(), name.clone());
            let clip = Arc::new(ClipSync::spawn(move |kind, data| {
                if clip_inner.lock().unwrap().active.as_deref() == Some(clip_name.as_str()) {
                    let _ = clip_tx.send(&Msg::Clip { kind: kind.to_string(), data });
                }
            }));

            let id = {
                let mut g = inner.lock().unwrap();
                let id = g.next_id;
                g.next_id += 1;
                if let Some(old) = g.pool.insert(
                    name.clone(),
                    Conn { id, tx: tx.clone(), ev: ev_tx.clone(), host_name: host_name.clone(), clip: clip.clone() },
                ) {
                    old.tx.close();
                }
                if g.epoch == my_epoch {
                    let _ = tx.send(&Msg::Claim);
                    g.active = Some(name.clone());
                    g.status = Status::Connected(host_name);
                    capture::set_sink(Some(ev_tx));
                    clip.resend();
                }
                id
            };

            let me = Client { inner: inner.clone() };
            while let Ok(msg) = rx.recv() {
                let is_active = inner.lock().unwrap().active.as_deref() == Some(name.as_str());
                match msg {
                    Msg::Clip { kind, data } if is_active => clip.apply(&kind, data),
                    Msg::Taken if is_active => {
                        // Другой компьютер взял управление: отпускаем ввод, соединение остаётся в запасе.
                        let mut g = inner.lock().unwrap();
                        if g.active.as_deref() == Some(name.as_str()) {
                            g.epoch += 1;
                            g.active = None;
                            capture::set_sink(None);
                            capture::log("управление перехвачено другим компьютером");
                            g.status = Status::Error("Управление перехвачено другим компьютером".into());
                        }
                    }
                    Msg::Switch { target } if is_active => {
                        let me = me.clone();
                        std::thread::spawn(move || crate::control::apply(&me, &target));
                    }
                    _ => {}
                }
            }
            let mut g = inner.lock().unwrap();
            if g.pool.get(&name).is_some_and(|c| c.id == id) {
                g.pool.remove(&name);
                if g.active.as_deref() == Some(name.as_str()) {
                    g.active = None;
                    capture::set_sink(None);
                    g.status = Status::Error("соединение закрыто".into());
                }
            }
        });
    }
}

fn open(addr: &str) -> std::io::Result<TcpStream> {
    let addr = addr.trim();
    let with_port = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    };
    let mut last = None;
    for sa in with_port.to_socket_addrs()? {
        match TcpStream::connect_timeout(&sa, Duration::from_secs(5)) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("адрес не найден")))
}

/// Склеивает подряд идущие движения мыши и прокрутки.
fn merge(batch: Vec<Ev>) -> Vec<Ev> {
    let mut out: Vec<Ev> = Vec::with_capacity(batch.len());
    for e in batch {
        match (out.last_mut(), &e) {
            (Some(Ev::Move { dx, dy }), Ev::Move { dx: ax, dy: ay }) => {
                *dx += ax;
                *dy += ay;
            }
            (Some(Ev::Wheel { v, h }), Ev::Wheel { v: av, h: ah }) if h == ah => *v += av,
            _ => out.push(e),
        }
    }
    out
}
