//! Синхронизация буфера обмена. Каждый тип данных - отдельная реализация `ClipFormat`,
//! так что добавить картинки или файлы можно, не трогая протокол и сетевой код:
//! достаточно написать ещё один формат и добавить его в `default_formats`.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

const MAX_CLIP: usize = 8 * 1024 * 1024;

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

fn default_formats() -> Vec<Box<dyn ClipFormat>> {
    let mut v: Vec<Box<dyn ClipFormat>> = Vec::new();
    if let Ok(cb) = arboard::Clipboard::new() {
        v.push(Box::new(TextFormat(cb)));
    }
    v
}

/// Фоновый поток, наблюдающий за буфером. Остановится, когда `ClipSync` будет уничтожен.
pub struct ClipSync {
    tx: Sender<(String, String)>,
}

impl ClipSync {
    /// `on_change(kind, data)` вызывается, когда пользователь скопировал что-то новое локально.
    pub fn spawn(on_change: impl Fn(&str, String) + Send + 'static) -> ClipSync {
        let (tx, rx) = mpsc::channel::<(String, String)>();
        std::thread::spawn(move || {
            let mut formats = default_formats();
            let mut last: HashMap<&'static str, String> = HashMap::new();
            let mut seq = unsafe { GetClipboardSequenceNumber() };
            loop {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok((kind, data)) => {
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

    /// Записать полученное от другой стороны в локальный буфер (без эха обратно).
    pub fn apply(&self, kind: &str, data: String) {
        let _ = self.tx.send((kind.to_string(), data));
    }
}
