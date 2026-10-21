//! Протокол switchdesk: бинарные кадры поверх TCP, без внешних зависимостей.
//!
//! Кадр = `u32 LE` длина тела + тело. Тело = тег (`u8`) + поля.
//! Строки: `u32 LE` длина + UTF-8. Все числа little-endian.

use std::io::{self, Read, Write};

pub const VERSION: u8 = 1;
/// Максимальный размер тела кадра.
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
/// Максимальный размер текста буфера обмена, который синхронизируется.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Button {
    Left = 0,
    Right = 1,
    Middle = 2,
    X1 = 3,
    X2 = 4,
}

impl Button {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Button::Left,
            1 => Button::Right,
            2 => Button::Middle,
            3 => Button::X1,
            4 => Button::X2,
            _ => return None,
        })
    }
}

/// Сообщения клиент → хост.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMsg {
    Hello { version: u8, name: String },
    /// Относительное смещение мыши (raw-дельты).
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: Button, down: bool },
    /// `delta` в единицах Windows (120 = один «щелчок»).
    Wheel { delta: i32, horizontal: bool },
    /// Сканкод + признак extended-клавиши.
    Key { scan: u16, extended: bool, up: bool },
    /// Отпустить все зажатые клавиши и кнопки.
    ReleaseAll,
    Clipboard(String),
    Ping(u32),
}

/// Сообщения хост → клиент.
#[derive(Debug, Clone, PartialEq)]
pub enum HostMsg {
    Welcome { version: u8, host_name: String },
    /// Вас вытеснил другой пользователь.
    Kicked { by: String },
    /// За хостом кто-то работает физически: удалённый ввод временно игнорируется.
    LocalActivity,
    Clipboard(String),
    Pong(u32),
    Rejected { reason: String },
}

pub trait Message: Sized {
    fn encode(&self, out: &mut Vec<u8>);
    fn decode(body: &[u8]) -> io::Result<Self>;
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Rd<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() - self.pos < n {
            return Err(bad("truncated frame"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| bad("invalid utf-8"))
    }
    fn finish(self) -> io::Result<()> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(bad("trailing bytes"))
        }
    }
}

impl Message for ClientMsg {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            ClientMsg::Hello { version, name } => {
                out.push(1);
                out.push(*version);
                put_str(out, name);
            }
            ClientMsg::MouseMove { dx, dy } => {
                out.push(2);
                out.extend_from_slice(&dx.to_le_bytes());
                out.extend_from_slice(&dy.to_le_bytes());
            }
            ClientMsg::MouseButton { button, down } => {
                out.push(3);
                out.push(*button as u8);
                out.push(*down as u8);
            }
            ClientMsg::Wheel { delta, horizontal } => {
                out.push(4);
                out.extend_from_slice(&delta.to_le_bytes());
                out.push(*horizontal as u8);
            }
            ClientMsg::Key { scan, extended, up } => {
                out.push(5);
                out.extend_from_slice(&scan.to_le_bytes());
                out.push(*extended as u8);
                out.push(*up as u8);
            }
            ClientMsg::ReleaseAll => out.push(6),
            ClientMsg::Clipboard(t) => {
                out.push(7);
                put_str(out, t);
            }
            ClientMsg::Ping(n) => {
                out.push(8);
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
    }

    fn decode(body: &[u8]) -> io::Result<Self> {
        let mut r = Rd { buf: body, pos: 0 };
        let msg = match r.u8()? {
            1 => ClientMsg::Hello { version: r.u8()?, name: r.string()? },
            2 => ClientMsg::MouseMove { dx: r.i32()?, dy: r.i32()? },
            3 => ClientMsg::MouseButton {
                button: Button::from_u8(r.u8()?).ok_or_else(|| bad("unknown button"))?,
                down: r.bool()?,
            },
            4 => ClientMsg::Wheel { delta: r.i32()?, horizontal: r.bool()? },
            5 => ClientMsg::Key { scan: r.u16()?, extended: r.bool()?, up: r.bool()? },
            6 => ClientMsg::ReleaseAll,
            7 => ClientMsg::Clipboard(r.string()?),
            8 => ClientMsg::Ping(r.u32()?),
            _ => return Err(bad("unknown client message")),
        };
        r.finish()?;
        Ok(msg)
    }
}

impl Message for HostMsg {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            HostMsg::Welcome { version, host_name } => {
                out.push(1);
                out.push(*version);
                put_str(out, host_name);
            }
            HostMsg::Kicked { by } => {
                out.push(2);
                put_str(out, by);
            }
            HostMsg::LocalActivity => out.push(3),
            HostMsg::Clipboard(t) => {
                out.push(4);
                put_str(out, t);
            }
            HostMsg::Pong(n) => {
                out.push(5);
                out.extend_from_slice(&n.to_le_bytes());
            }
            HostMsg::Rejected { reason } => {
                out.push(6);
                put_str(out, reason);
            }
        }
    }

    fn decode(body: &[u8]) -> io::Result<Self> {
        let mut r = Rd { buf: body, pos: 0 };
        let msg = match r.u8()? {
            1 => HostMsg::Welcome { version: r.u8()?, host_name: r.string()? },
            2 => HostMsg::Kicked { by: r.string()? },
            3 => HostMsg::LocalActivity,
            4 => HostMsg::Clipboard(r.string()?),
            5 => HostMsg::Pong(r.u32()?),
            6 => HostMsg::Rejected { reason: r.string()? },
            _ => return Err(bad("unknown host message")),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// Дописывает в `out` один готовый кадр (длина + тело).
pub fn encode_frame<M: Message>(msg: &M, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    msg.encode(out);
    let len = (out.len() - start - 4) as u32;
    out[start..start + 4].copy_from_slice(&len.to_le_bytes());
}

/// Отправляет одно сообщение одним `write_all`.
pub fn write_msg<M: Message, W: Write>(w: &mut W, msg: &M) -> io::Result<()> {
    let mut buf = Vec::with_capacity(32);
    encode_frame(msg, &mut buf);
    w.write_all(&buf)
}

/// Блокирующее чтение одного сообщения.
pub fn read_msg<M: Message, R: Read>(r: &mut R) -> io::Result<M> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(bad("bad frame length"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    M::decode(&body)
}

/// Склеивает подряд идущие `MouseMove` и `Wheel` (одной ориентации),
/// не меняя порядок остальных событий.
pub fn coalesce(msgs: Vec<ClientMsg>) -> Vec<ClientMsg> {
    let mut out: Vec<ClientMsg> = Vec::with_capacity(msgs.len());
    for m in msgs {
        match (out.last_mut(), &m) {
            (Some(ClientMsg::MouseMove { dx, dy }), ClientMsg::MouseMove { dx: ax, dy: ay }) => {
                *dx = dx.saturating_add(*ax);
                *dy = dy.saturating_add(*ay);
            }
            (
                Some(ClientMsg::Wheel { delta, horizontal }),
                ClientMsg::Wheel { delta: ad, horizontal: ah },
            ) if horizontal == ah => {
                *delta = delta.saturating_add(*ad);
            }
            _ => out.push(m),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn roundtrip_client(m: ClientMsg) {
        let mut buf = Vec::new();
        encode_frame(&m, &mut buf);
        let got: ClientMsg = read_msg(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, m);
    }

    fn roundtrip_host(m: HostMsg) {
        let mut buf = Vec::new();
        encode_frame(&m, &mut buf);
        let got: HostMsg = read_msg(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, m);
    }

    #[test]
    fn client_messages_roundtrip() {
        roundtrip_client(ClientMsg::Hello { version: VERSION, name: "Даниил".into() });
        roundtrip_client(ClientMsg::MouseMove { dx: -5, dy: 1000 });
        roundtrip_client(ClientMsg::MouseButton { button: Button::X2, down: true });
        roundtrip_client(ClientMsg::Wheel { delta: -120, horizontal: true });
        roundtrip_client(ClientMsg::Key { scan: 0x1D, extended: true, up: false });
        roundtrip_client(ClientMsg::ReleaseAll);
        roundtrip_client(ClientMsg::Clipboard("привет\nworld".into()));
        roundtrip_client(ClientMsg::Ping(42));
    }

    #[test]
    fn host_messages_roundtrip() {
        roundtrip_host(HostMsg::Welcome { version: VERSION, host_name: "srv".into() });
        roundtrip_host(HostMsg::Kicked { by: "Коллега".into() });
        roundtrip_host(HostMsg::LocalActivity);
        roundtrip_host(HostMsg::Clipboard("x".into()));
        roundtrip_host(HostMsg::Pong(7));
        roundtrip_host(HostMsg::Rejected { reason: "версия".into() });
    }

    #[test]
    fn rejects_oversized_and_empty_frames() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0; 8]);
        assert!(read_msg::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
        let zero = 0u32.to_le_bytes().to_vec();
        assert!(read_msg::<ClientMsg, _>(&mut Cursor::new(zero)).is_err());
    }

    #[test]
    fn rejects_garbage() {
        // неизвестный тег
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.push(99);
        assert!(read_msg::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
        // строка длиннее тела
        let mut buf = Vec::new();
        buf.extend_from_slice(&6u32.to_le_bytes());
        buf.extend_from_slice(&[7, 0xFF, 0xFF, 0xFF, 0x7F, b'a']);
        assert!(read_msg::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
        // лишние байты после сообщения
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&[6, 0]);
        assert!(read_msg::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn coalesce_merges_only_adjacent_moves_and_wheels() {
        let key = ClientMsg::Key { scan: 1, extended: false, up: false };
        let out = coalesce(vec![
            ClientMsg::MouseMove { dx: 1, dy: 2 },
            ClientMsg::MouseMove { dx: 3, dy: -4 },
            key.clone(),
            ClientMsg::MouseMove { dx: 5, dy: 5 },
            ClientMsg::Wheel { delta: 120, horizontal: false },
            ClientMsg::Wheel { delta: 120, horizontal: false },
            ClientMsg::Wheel { delta: 120, horizontal: true },
        ]);
        assert_eq!(
            out,
            vec![
                ClientMsg::MouseMove { dx: 4, dy: -2 },
                key,
                ClientMsg::MouseMove { dx: 5, dy: 5 },
                ClientMsg::Wheel { delta: 240, horizontal: false },
                ClientMsg::Wheel { delta: 120, horizontal: true },
            ]
        );
    }
}
