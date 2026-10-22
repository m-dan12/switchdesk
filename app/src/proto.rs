//! Протокол: зашифрованные кадры поверх TCP.
//!
//! Рукопожатие: хост шлёт 16 байт nonce, клиент отвечает своими 16. Ключ AES-256-GCM
//! выводится из общего токена (PBKDF2) и обоих nonce. Любой кадр, не прошедший
//! расшифровку, закрывает соединение, поэтому отдельной проверки пароля нет:
//! неверный токен = первый же кадр не расшифруется.
//! Кадр: u32 (big endian) длина + шифртекст. Nonce AEAD = направление + счётчик кадров,
//! счётчик строго возрастает, поэтому повтор или перестановка кадров отвергаются.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Событие ввода (клиент -> хост).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "k")]
pub enum Ev {
    Move { dx: i32, dy: i32 },
    /// b: 0 левая, 1 правая, 2 средняя, 3 X1, 4 X2
    Btn { b: u8, down: bool },
    Wheel { v: i32, h: bool },
    /// Скан-код (не зависит от раскладки) + признак extended.
    Key { sc: u16, ext: bool, down: bool },
    /// Отпустить все нажатые клавиши и кнопки этого клиента.
    Release,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t")]
pub enum Msg {
    Hello { name: String },
    Welcome { name: String },
    Events { e: Vec<Ev> },
    /// Буфер обмена. kind: "text" (потом "image", "files"...), data: содержимое
    /// (текст как есть, бинарные форматы в base64).
    Clip { kind: String, data: String },
}

fn err(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn derive(token: &str, ns: &[u8; 16], nc: &[u8; 16]) -> [u8; 32] {
    let mut salt = b"switchdesk1".to_vec();
    salt.extend_from_slice(ns);
    salt.extend_from_slice(nc);
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(token.as_bytes(), &salt, 100_000, &mut key);
    key
}

fn nonce(dir: u8, ctr: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = dir;
    n[4..].copy_from_slice(&ctr.to_be_bytes());
    n
}

struct TxInner {
    stream: TcpStream,
    cipher: Aes256Gcm,
    dir: u8,
    ctr: u64,
}

/// Отправляющая половина; клонируется и безопасна для нескольких потоков.
#[derive(Clone)]
pub struct Tx {
    inner: Arc<Mutex<TxInner>>,
    sock: Arc<TcpStream>,
}

impl Tx {
    pub fn send(&self, m: &Msg) -> io::Result<()> {
        let data = serde_json::to_vec(m).map_err(|_| err("serialize"))?;
        let mut g = self.inner.lock().unwrap();
        let n = nonce(g.dir, g.ctr);
        let ct = g
            .cipher
            .encrypt(Nonce::from_slice(&n), data.as_slice())
            .map_err(|_| err("encrypt"))?;
        g.ctr += 1;
        let mut buf = (ct.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(&ct);
        g.stream.write_all(&buf)
    }

    /// Закрыть соединение (разбудит поток, читающий Rx).
    pub fn close(&self) {
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

pub struct Rx {
    stream: TcpStream,
    cipher: Aes256Gcm,
    dir: u8,
    ctr: u64,
}

impl Rx {
    pub fn recv(&mut self) -> io::Result<Msg> {
        let mut hdr = [0u8; 4];
        self.stream.read_exact(&mut hdr)?;
        let n = u32::from_be_bytes(hdr) as usize;
        if n > MAX_FRAME {
            return Err(err("frame too large"));
        }
        let mut ct = vec![0u8; n];
        self.stream.read_exact(&mut ct)?;
        let nn = nonce(self.dir, self.ctr);
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(&nn), ct.as_slice())
            .map_err(|_| err("bad token or corrupted frame"))?;
        self.ctr += 1;
        serde_json::from_slice(&pt).map_err(|_| err("bad message"))
    }
}

fn pair(stream: TcpStream, key: [u8; 32], is_server: bool) -> io::Result<(Tx, Rx)> {
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| err("key"))?;
    let (sd, rd) = if is_server { (b'S', b'C') } else { (b'C', b'S') };
    let tx = Tx {
        inner: Arc::new(Mutex::new(TxInner {
            stream: stream.try_clone()?,
            cipher: cipher.clone(),
            dir: sd,
            ctr: 0,
        })),
        sock: Arc::new(stream.try_clone()?),
    };
    let rx = Rx { stream, cipher, dir: rd, ctr: 0 };
    Ok((tx, rx))
}

/// Сторона хоста. Возвращает (передатчик, приёмник, имя клиента).
pub fn server_handshake(
    mut s: TcpStream,
    token: &str,
    host_name: &str,
) -> io::Result<(Tx, Rx, String)> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut ns = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut ns);
    s.write_all(&ns)?;
    let mut nc = [0u8; 16];
    s.read_exact(&mut nc)?;
    let (tx, mut rx) = pair(s, derive(token, &ns, &nc), true)?;
    let name = match rx.recv()? {
        Msg::Hello { name } => name,
        _ => return Err(err("expected hello")),
    };
    tx.send(&Msg::Welcome { name: host_name.to_string() })?;
    rx.stream.set_read_timeout(None)?;
    Ok((tx, rx, name))
}

/// Сторона клиента. Возвращает (передатчик, приёмник, имя хоста).
pub fn client_handshake(
    mut s: TcpStream,
    token: &str,
    my_name: &str,
) -> io::Result<(Tx, Rx, String)> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(8)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut ns = [0u8; 16];
    s.read_exact(&mut ns)?;
    let mut nc = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nc);
    s.write_all(&nc)?;
    let (tx, mut rx) = pair(s, derive(token, &ns, &nc), false)?;
    tx.send(&Msg::Hello { name: my_name.to_string() })?;
    let name = match rx.recv() {
        Ok(Msg::Welcome { name }) => name,
        Ok(_) => return Err(err("unexpected reply")),
        // Хост закрывает соединение, если токен не подошёл.
        Err(_) => return Err(err("отказ: неверный токен или хост недоступен")),
    };
    rx.stream.set_read_timeout(None)?;
    Ok((tx, rx, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn run(server_token: &'static str, client_token: &str) -> io::Result<()> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let (tx, mut rx, name) = server_handshake(s, server_token, "host")?;
            assert_eq!(name, "cli");
            match rx.recv()? {
                Msg::Events { e } => tx.send(&Msg::Events { e })?,
                _ => panic!(),
            }
            Ok::<_, io::Error>(())
        });
        let s = TcpStream::connect(addr).unwrap();
        let (tx, mut rx, host) = client_handshake(s, client_token, "cli")?;
        assert_eq!(host, "host");
        let evs = vec![Ev::Move { dx: 3, dy: -4 }, Ev::Key { sc: 30, ext: false, down: true }];
        tx.send(&Msg::Events { e: evs.clone() })?;
        match rx.recv()? {
            Msg::Events { e } => assert_eq!(e, evs),
            _ => panic!(),
        }
        h.join().unwrap().unwrap();
        Ok(())
    }

    #[test]
    fn good_token() {
        run("secret", "secret").unwrap();
    }

    #[test]
    fn bad_token_rejected() {
        assert!(run("secret", "wrong").is_err());
    }
}
