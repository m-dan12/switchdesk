//! Связь окна (`--gui`) с фоновым ядром: JSON-строки по TCP на 127.0.0.1.
//! На каждый запрос ядро отвечает свежим состоянием.

use crate::client::Status;
use crate::config::Device;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

const PORT: u16 = 52802;

#[derive(Serialize, Deserialize)]
#[serde(tag = "c")]
pub enum Req {
    State,
    Connect { dev: Device, me: String },
    Disconnect,
    /// Передать ввод удалённому ПК / вернуть себе.
    Toggle,
    /// Перечитать настройки хоста из config.json и применить.
    HostApply,
    UpdateCheck,
    UpdateInstall,
    RestartElevated,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub enum UpdView {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(String),
    Downloading(String),
    Error(String),
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ClientView {
    pub name: String,
    pub ip: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct State {
    pub status: Status,
    /// Ввод сейчас передаётся на удалённый ПК.
    pub active: bool,
    pub host_running: bool,
    pub host_error: Option<String>,
    pub clients: Vec<ClientView>,
    pub update: UpdView,
    pub elevated: bool,
}

pub fn serve(handler: impl Fn(Req) -> State + Send + Sync + 'static) {
    let Ok(listener) = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))) else { return };
    let handler = std::sync::Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = handler.clone();
            std::thread::spawn(move || {
                let Ok(mut out) = stream.try_clone() else { return };
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let Ok(req) = serde_json::from_str::<Req>(&line) else { continue };
                    let Ok(mut resp) = serde_json::to_string(&handler(req)) else { continue };
                    resp.push('\n');
                    if out.write_all(resp.as_bytes()).is_err() {
                        break;
                    }
                }
            });
        }
    });
}

pub fn call(req: &Req) -> std::io::Result<State> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut line = serde_json::to_string(req).map_err(std::io::Error::other)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut resp = String::new();
    BufReader::new(s).read_line(&mut resp)?;
    serde_json::from_str(&resp).map_err(std::io::Error::other)
}
