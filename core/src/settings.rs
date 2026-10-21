//! Настройки приложения (хранятся в JSON рядом с профилем пользователя).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 7878;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Peer {
    pub name: String,
    /// `имя-или-ip` или `имя-или-ip:порт`.
    pub addr: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Как этот компьютер называется для других.
    pub device_name: String,
    /// Компьютеры, на которые можно переключаться.
    pub peers: Vec<Peer>,
    /// Имя компьютера из `peers`, на который переключает горячая клавиша.
    pub target: String,
    /// Принимать ввод от других компьютеров.
    pub accept_input: bool,
    /// `auto` — адрес Tailscale этого компьютера, иначе явный адрес (`ip` или `ip:порт`).
    pub listen: String,
    pub port: u16,
    pub autostart: bool,
    pub auto_update: bool,
    /// Пауза удалённого ввода после физического ввода за этим компьютером.
    pub grace_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Мой компьютер".into()),
            peers: Vec::new(),
            target: String::new(),
            accept_input: true,
            listen: "auto".into(),
            port: DEFAULT_PORT,
            autostart: true,
            auto_update: true,
            grace_ms: 1500,
        }
    }
}

impl Settings {
    /// Выбранный компьютер; если выбор пропал, но компьютер единственный — он.
    pub fn target_peer(&self) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|p| p.name == self.target)
            .or_else(|| if self.peers.len() == 1 { self.peers.first() } else { None })
    }

    /// Приводит значения к допустимым (после ручной правки файла или ввода).
    pub fn sanitize(&mut self) {
        self.device_name = self.device_name.chars().filter(|c| !c.is_control()).take(64).collect();
        if self.device_name.trim().is_empty() {
            self.device_name = Settings::default().device_name;
        }
        if self.port == 0 {
            self.port = DEFAULT_PORT;
        }
        self.peers.retain(|p| !p.name.trim().is_empty() && normalize_addr(&p.addr).is_some());
        for p in &mut self.peers {
            p.addr = normalize_addr(&p.addr).unwrap_or_default();
        }
        if self.listen.trim().is_empty() {
            self.listen = "auto".into();
        }
    }
}

/// Проверяет адрес и добавляет порт по умолчанию.
pub fn normalize_addr(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
        return None;
    }
    let port_ok = |p: &str| p.parse::<u16>().is_ok_and(|p| p != 0);
    if let Some(rest) = s.strip_prefix('[') {
        // [::1] или [::1]:7878
        let (host, tail) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        return match tail {
            "" => Some(format!("[{host}]:{DEFAULT_PORT}")),
            t => t.strip_prefix(':').filter(|p| port_ok(p)).map(|_| s.to_owned()),
        };
    }
    match s.rsplit_once(':') {
        None => Some(format!("{s}:{DEFAULT_PORT}")),
        Some((host, port)) if !host.is_empty() && !host.contains(':') && port_ok(port) => Some(s.to_owned()),
        _ => None,
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("Switchdesk")
}

fn path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    let mut s: Settings = std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let json = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    // запись через временный файл, чтобы не оставить половину настроек при сбое
    let tmp = path().with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize_addr("pc.tail.ts.net").as_deref(), Some("pc.tail.ts.net:7878"));
        assert_eq!(normalize_addr(" 100.1.2.3:9000 ").as_deref(), Some("100.1.2.3:9000"));
        assert_eq!(normalize_addr("[fd7a::1]").as_deref(), Some("[fd7a::1]:7878"));
        assert_eq!(normalize_addr("[fd7a::1]:81").as_deref(), Some("[fd7a::1]:81"));
        for bad in ["", "  ", "a b", "host:0", "host:99999", "host:", ":7878", "a:b:c", "http://x", "[]"] {
            assert_eq!(normalize_addr(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn partial_json_uses_defaults() {
        let s: Settings = serde_json::from_str(r#"{"device_name":"PC","peers":[{"name":"A","addr":"x"}]}"#).unwrap();
        assert_eq!(s.device_name, "PC");
        assert_eq!(s.port, DEFAULT_PORT);
        assert!(s.accept_input && s.auto_update);
        assert_eq!(s.peers[0].name, "A");
    }

    #[test]
    fn sanitize_drops_bad_peers_and_adds_port() {
        let mut s = Settings {
            peers: vec![
                Peer { name: "ok".into(), addr: "10.0.0.2".into() },
                Peer { name: " ".into(), addr: "10.0.0.3".into() },
                Peer { name: "bad".into(), addr: "a b".into() },
            ],
            device_name: "\u{7}".into(),
            port: 0,
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.peers, vec![Peer { name: "ok".into(), addr: "10.0.0.2:7878".into() }]);
        assert_eq!(s.port, DEFAULT_PORT);
        assert!(!s.device_name.trim().is_empty());
    }

    #[test]
    fn target_falls_back_to_single_peer() {
        let mut s = Settings::default();
        assert!(s.target_peer().is_none());
        s.peers.push(Peer { name: "A".into(), addr: "a:1".into() });
        assert_eq!(s.target_peer().unwrap().name, "A");
        s.peers.push(Peer { name: "B".into(), addr: "b:1".into() });
        assert!(s.target_peer().is_none());
        s.target = "B".into();
        assert_eq!(s.target_peer().unwrap().name, "B");
    }
}
