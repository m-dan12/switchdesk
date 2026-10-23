//! Клиентская сторона: соединение с одним выбранным устройством.

use crate::capture;
use crate::clip::ClipSync;
use crate::config::{Device, DEFAULT_PORT};
use crate::proto::{self, Ev, Msg, Tx};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, PartialEq, Debug)]
pub enum Status {
    Idle,
    Connecting,
    Connected(String),
    Error(String),
}

#[derive(Clone)]
pub struct Client {
    pub status: Arc<Mutex<Status>>,
    closer: Arc<Mutex<Option<Tx>>>,
    /// Номер текущей попытки: устаревшие потоки не должны менять статус.
    epoch: Arc<AtomicU64>,
    /// Имя устройства последнего подключения.
    target: Arc<Mutex<Option<String>>>,
}

impl Client {
    pub fn new() -> Client {
        Client {
            status: Arc::new(Mutex::new(Status::Idle)),
            closer: Arc::new(Mutex::new(None)),
            epoch: Arc::new(AtomicU64::new(0)),
            target: Arc::new(Mutex::new(None)),
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn target(&self) -> Option<String> {
        self.target.lock().unwrap().clone()
    }

    pub fn disconnect(&self) {
        *self.target.lock().unwrap() = None;
        self.epoch.fetch_add(1, SeqCst);
        if let Some(tx) = self.closer.lock().unwrap().take() {
            tx.close();
        }
        capture::set_sink(None);
        *self.status.lock().unwrap() = Status::Idle;
    }

    /// Подключение к устройству (предыдущее соединение закрывается).
    pub fn connect(&self, dev: Device, my_name: String) {
        self.disconnect();
        let my_epoch = self.epoch.load(SeqCst);
        *self.target.lock().unwrap() = Some(dev.name.clone());
        *self.status.lock().unwrap() = Status::Connecting;
        let me = self.clone();
        let (status, closer, epoch) = (self.status.clone(), self.closer.clone(), self.epoch.clone());
        std::thread::spawn(move || {
            let current = || epoch.load(SeqCst) == my_epoch;
            let set = |s: Status| {
                if current() {
                    *status.lock().unwrap() = s;
                }
            };
            let stream = match open(&dev.addr) {
                Ok(s) => s,
                Err(e) => return set(Status::Error(format!("не удалось подключиться: {e}"))),
            };
            let (tx, mut rx, host_name) = match proto::client_handshake(stream, dev.token.trim(), &my_name) {
                Ok(v) => v,
                Err(e) => return set(Status::Error(e.to_string())),
            };
            if !current() {
                tx.close();
                return;
            }
            *closer.lock().unwrap() = Some(tx.clone());

            // События ввода -> хост (с объединением движений мыши).
            let (ev_tx, ev_rx) = mpsc::channel::<Ev>();
            let sender_tx = tx.clone();
            std::thread::spawn(move || {
                while let Ok(first) = ev_rx.recv() {
                    let mut batch = vec![first];
                    while batch.len() < 512 {
                        match ev_rx.try_recv() {
                            Ok(e) => batch.push(e),
                            Err(_) => break,
                        }
                    }
                    if sender_tx.send(&Msg::Events { e: merge(batch) }).is_err() {
                        break;
                    }
                }
            });
            // Буфер обмена в обе стороны.
            let clip_tx = tx.clone();
            let clip = ClipSync::spawn(move |kind, data| {
                let _ = clip_tx.send(&Msg::Clip { kind: kind.to_string(), data });
            });

            capture::set_sink(Some(ev_tx));
            set(Status::Connected(host_name));

            while let Ok(msg) = rx.recv() {
                match msg {
                    Msg::Clip { kind, data } => clip.apply(&kind, data),
                    Msg::Switch { target } => {
                        let me = me.clone();
                        std::thread::spawn(move || crate::control::apply(&me, &target));
                    }
                    _ => {}
                }
            }
            if current() {
                capture::set_sink(None);
                *closer.lock().unwrap() = None;
                set(Status::Error("соединение закрыто".into()));
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
