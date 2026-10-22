//! Хост: принимает подключения нескольких клиентов одновременно и применяет их ввод.

use crate::clip::ClipSync;
use crate::inject;
use crate::proto::{self, Ev, Msg, Tx};
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct ClientInfo {
    pub id: u64,
    pub name: String,
    pub addr: SocketAddr,
}

/// Состояние, видимое интерфейсу.
pub struct HostShared {
    pub allow_input: AtomicBool,
    pub clients: Mutex<Vec<ClientInfo>>,
}

impl HostShared {
    pub fn new(allow_input: bool) -> Arc<HostShared> {
        Arc::new(HostShared { allow_input: AtomicBool::new(allow_input), clients: Mutex::new(Vec::new()) })
    }
}

pub struct Host {
    stop: Arc<AtomicBool>,
    txs: Arc<Mutex<HashMap<u64, Tx>>>,
}

impl Host {
    pub fn start(port: u16, token: String, host_name: String, shared: Arc<HostShared>) -> std::io::Result<Host> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let txs: Arc<Mutex<HashMap<u64, Tx>>> = Arc::new(Mutex::new(HashMap::new()));
        // Содержимое буфера хоста раздаётся всем клиентам.
        let clip = {
            let txs = txs.clone();
            Arc::new(ClipSync::spawn(move |kind, data| broadcast(&txs, None, kind, &data)))
        };
        let (stop2, txs2) = (stop.clone(), txs.clone());
        std::thread::spawn(move || {
            let next_id = AtomicU64::new(1);
            while !stop2.load(SeqCst) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        let _ = stream.set_nonblocking(false);
                        let id = next_id.fetch_add(1, SeqCst);
                        let (token, name, shared, txs, clip, stop) =
                            (token.clone(), host_name.clone(), shared.clone(), txs2.clone(), clip.clone(), stop2.clone());
                        std::thread::spawn(move || serve(id, stream, addr, token, name, shared, txs, clip, stop));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Ok(Host { stop, txs })
    }

    pub fn stop(&self) {
        self.stop.store(true, SeqCst);
        for tx in self.txs.lock().unwrap().values() {
            tx.close();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

fn broadcast(txs: &Mutex<HashMap<u64, Tx>>, except: Option<u64>, kind: &str, data: &str) {
    let targets: Vec<Tx> = txs
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, _)| Some(**id) != except)
        .map(|(_, t)| t.clone())
        .collect();
    for t in targets {
        let _ = t.send(&Msg::Clip { kind: kind.to_string(), data: data.to_string() });
    }
}

#[derive(Default)]
struct Pressed {
    keys: HashSet<(u16, bool)>,
    buttons: HashSet<u8>,
}

impl Pressed {
    fn release_all(&mut self) {
        for (sc, ext) in self.keys.drain() {
            inject::key(sc, ext, false);
        }
        for b in self.buttons.drain() {
            inject::button(b, false);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn serve(
    id: u64,
    stream: std::net::TcpStream,
    addr: SocketAddr,
    token: String,
    host_name: String,
    shared: Arc<HostShared>,
    txs: Arc<Mutex<HashMap<u64, Tx>>>,
    clip: Arc<ClipSync>,
    stop: Arc<AtomicBool>,
) {
    let Ok((tx, mut rx, name)) = proto::server_handshake(stream, &token, &host_name) else {
        return;
    };
    if stop.load(SeqCst) {
        return;
    }
    txs.lock().unwrap().insert(id, tx);
    shared.clients.lock().unwrap().push(ClientInfo { id, name, addr });

    let mut pressed = Pressed::default();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Events { e } => {
                if !shared.allow_input.load(SeqCst) {
                    pressed.release_all();
                    continue;
                }
                for ev in e {
                    match ev {
                        Ev::Move { dx, dy } => inject::mouse_move(dx, dy),
                        Ev::Btn { b, down } => {
                            if down {
                                pressed.buttons.insert(b);
                            } else {
                                pressed.buttons.remove(&b);
                            }
                            inject::button(b, down);
                        }
                        Ev::Wheel { v, h } => inject::wheel(v, h),
                        Ev::Key { sc, ext, down } => {
                            crate::capture::host_log(&format!("host key sc={sc:#04x} ext={ext} down={down} from {}", addr.ip()));
                            if down {
                                pressed.keys.insert((sc, ext));
                            } else {
                                pressed.keys.remove(&(sc, ext));
                            }
                            inject::key(sc, ext, down);
                        }
                        Ev::Release => pressed.release_all(),
                    }
                }
            }
            Msg::Clip { kind, data } => {
                // Применяем у себя и пересылаем остальным клиентам.
                clip.apply(&kind, data.clone());
                broadcast(&txs, Some(id), &kind, &data);
            }
            _ => {}
        }
    }
    pressed.release_all();
    txs.lock().unwrap().remove(&id);
    shared.clients.lock().unwrap().retain(|c| c.id != id);
}
