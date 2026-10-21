//! Общее состояние для интерфейса: режим, кто управляет, журнал, обновления.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const MAX_LOG: usize = 200;
const MAX_LOG_FILE: u64 = 512 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// Ввод остаётся на этом компьютере.
    Local,
    Connecting(String),
    /// Ваш ввод уходит на компьютер с таким именем.
    Remote(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Listen {
    Off,
    On(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Unknown,
    Checking,
    UpToDate,
    Available(String),
    Installing,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub mode: Mode,
    /// Имена тех, кто сейчас подключён к этому компьютеру.
    pub hosting: Vec<String>,
    pub listen: Listen,
    pub update: UpdateState,
    pub logs: Vec<String>,
    /// Растёт при любом изменении — чтобы дёшево понять, что перерисовывать.
    pub generation: u64,
}

struct State {
    snap: Snapshot,
    logs: VecDeque<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static REPAINT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static LOG_FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| State {
        snap: Snapshot {
            mode: Mode::Local,
            hosting: Vec::new(),
            listen: Listen::Off,
            update: UpdateState::Unknown,
            logs: Vec::new(),
            generation: 0,
        },
        logs: VecDeque::new(),
    });
    f(st)
}

fn changed(st: &mut State) {
    st.snap.generation += 1;
}

fn repaint() {
    if let Some(f) = REPAINT.get() {
        f();
    }
}

/// Колбэк «перерисуй интерфейс». Задаётся один раз.
pub fn set_repaint(f: impl Fn() + Send + Sync + 'static) {
    let _ = REPAINT.set(Box::new(f));
}

pub fn set_log_file(path: std::path::PathBuf) {
    *LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

pub fn snapshot() -> Snapshot {
    with(|st| {
        let mut s = st.snap.clone();
        s.logs = st.logs.iter().cloned().collect();
        s
    })
}

pub fn generation() -> u64 {
    with(|st| st.snap.generation)
}

fn set<T: PartialEq>(slot: impl FnOnce(&mut Snapshot) -> &mut T, v: T) {
    let did = with(|st| {
        let s = slot(&mut st.snap);
        if *s == v {
            return false;
        }
        *s = v;
        changed(st);
        true
    });
    if did {
        repaint();
    }
}

pub fn set_mode(m: Mode) {
    set(|s| &mut s.mode, m);
}
pub fn set_hosting(h: Vec<String>) {
    set(|s| &mut s.hosting, h);
}
pub fn set_listen(l: Listen) {
    set(|s| &mut s.listen, l);
}
pub fn set_update(u: UpdateState) {
    set(|s| &mut s.update, u);
}

#[cfg(windows)]
fn timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

#[cfg(not(windows))]
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

fn append_file(line: &str) {
    let Some(path) = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_FILE) {
        let _ = std::fs::remove_file(&path);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Строка в журнал (видна в интерфейсе и пишется в файл).
pub fn log(msg: impl AsRef<str>) {
    let line = format!("{}  {}", timestamp(), msg.as_ref());
    append_file(&line);
    with(|st| {
        if st.logs.len() >= MAX_LOG {
            st.logs.pop_front();
        }
        st.logs.push_back(line);
        changed(st);
    });
    repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Состояние глобальное, а тесты идут параллельно.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mode_changes_bump_generation_once() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let g0 = generation();
        set_mode(Mode::Remote("PC".into()));
        let g1 = generation();
        assert_eq!(g1, g0 + 1);
        set_mode(Mode::Remote("PC".into()));
        assert_eq!(generation(), g1, "то же значение не считается изменением");
        assert_eq!(snapshot().mode, Mode::Remote("PC".into()));
        set_mode(Mode::Local);
    }

    #[test]
    fn log_is_capped() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..(MAX_LOG + 20) {
            log(format!("строка {i}"));
        }
        let s = snapshot();
        assert_eq!(s.logs.len(), MAX_LOG);
        assert!(s.logs.last().unwrap().ends_with(&format!("строка {}", MAX_LOG + 19)));
    }
}
