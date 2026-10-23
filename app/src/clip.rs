//! Синхронизация буфера обмена. Каждый тип данных - отдельная реализация `ClipFormat`,
//! так что добавить картинки или файлы можно, не трогая протокол и сетевой код:
//! достаточно написать ещё один формат и добавить его в `default_formats`.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

const MAX_CLIP: usize = 8 * 1024 * 1024;
/// Предел картинки (base64): кадр протокола не больше 16 МБ.
const MAX_IMAGE: usize = 12 * 1024 * 1024;

pub trait ClipFormat {
    fn kind(&self) -> &'static str;
    /// Текущее содержимое буфера в этом формате (None, если там другой тип данных).
    fn read(&mut self) -> Option<String>;
    fn write(&mut self, data: &str);
}

struct TextFormat(arboard::Clipboard);

impl ClipFormat for TextFormat {
    fn kind(&self) -> &'static str {
        "text"
    }
    fn read(&mut self) -> Option<String> {
        self.0.get_text().ok().filter(|t| !t.is_empty() && t.len() <= MAX_CLIP)
    }
    fn write(&mut self, data: &str) {
        let _ = self.0.set_text(data.to_string());
    }
}

/// Картинки (скриншоты): в канале PNG в base64.
struct ImageFormat(arboard::Clipboard);

impl ClipFormat for ImageFormat {
    fn kind(&self) -> &'static str {
        "image"
    }
    fn read(&mut self) -> Option<String> {
        let img = self.0.get_image().ok()?;
        let png = encode_png(img.width as u32, img.height as u32, &img.bytes)?;
        let b64 = BASE64.encode(png);
        (b64.len() <= MAX_IMAGE).then_some(b64)
    }
    fn write(&mut self, data: &str) {
        let Some((w, h, rgba)) = BASE64.decode(data).ok().and_then(|b| decode_png(&b)) else { return };
        let _ = self.0.set_image(arboard::ImageData { width: w, height: h, bytes: rgba.into() });
    }
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut wr = enc.write_header().ok()?;
    wr.write_image_data(rgba).ok()?;
    wr.finish().ok()?;
    Some(out)
}

fn decode_png(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width as usize, info.height as usize, rgba))
}

fn default_formats() -> Vec<Box<dyn ClipFormat>> {
    let mut v: Vec<Box<dyn ClipFormat>> = Vec::new();
    if let Ok(cb) = arboard::Clipboard::new() {
        v.push(Box::new(TextFormat(cb)));
    }
    if let Ok(cb) = arboard::Clipboard::new() {
        v.push(Box::new(ImageFormat(cb)));
    }
    v
}

enum Cmd {
    Apply(String, String),
    /// Забыть, что уже отправлено, и отправить текущее содержимое заново.
    Resend,
}

/// Фоновый поток, наблюдающий за буфером. Остановится, когда `ClipSync` будет уничтожен.
pub struct ClipSync {
    tx: Sender<Cmd>,
}

impl ClipSync {
    /// `on_change(kind, data)` вызывается, когда пользователь скопировал что-то новое локально.
    pub fn spawn(on_change: impl Fn(&str, String) + Send + 'static) -> ClipSync {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::spawn(move || {
            let mut formats = default_formats();
            let mut last: HashMap<&'static str, String> = HashMap::new();
            let mut seq = unsafe { GetClipboardSequenceNumber() };
            loop {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(Cmd::Resend) => {
                        last.clear();
                        seq = 0;
                        continue;
                    }
                    Ok(Cmd::Apply(kind, data)) => {
                        for f in formats.iter_mut().filter(|f| f.kind() == kind) {
                            last.insert(f.kind(), data.clone());
                            f.write(&data);
                        }
                        seq = unsafe { GetClipboardSequenceNumber() };
                        continue;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let now = unsafe { GetClipboardSequenceNumber() };
                if now == seq {
                    continue;
                }
                seq = now;
                for f in formats.iter_mut() {
                    if let Some(d) = f.read() {
                        if last.get(f.kind()) != Some(&d) {
                            last.insert(f.kind(), d.clone());
                            on_change(f.kind(), d);
                        }
                    }
                }
            }
        });
        ClipSync { tx }
    }

    /// Отправить текущее содержимое буфера заново (устройство стало активным).
    pub fn resend(&self) {
        let _ = self.tx.send(Cmd::Resend);
    }

    /// Записать полученное от другой стороны в локальный буфер (без эха обратно).
    pub fn apply(&self, kind: &str, data: String) {
        let _ = self.tx.send(Cmd::Apply(kind.to_string(), data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip() {
        let rgba: Vec<u8> = (0..4 * 3 * 4).map(|i| (i * 7) as u8).collect();
        let png = encode_png(4, 3, &rgba).unwrap();
        let (w, h, back) = decode_png(&png).unwrap();
        assert_eq!((w, h), (4, 3));
        assert_eq!(back, rgba);
    }
}
