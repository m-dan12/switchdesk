use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 52800;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Device {
    pub name: String,
    /// host:port (порт можно не указывать)
    pub addr: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub pc_name: String,
    pub devices: Vec<Device>,
    pub last_device: usize,
    pub host_enabled: bool,
    pub host_port: u16,
    pub host_token: String,
    pub host_allow_input: bool,
    /// "system" | "light" | "dark"
    pub theme: String,
}

pub fn new_token() -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(24).map(char::from).collect()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            pc_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()),
            devices: Vec::new(),
            last_device: 0,
            host_enabled: false,
            host_port: DEFAULT_PORT,
            host_token: new_token(),
            host_allow_input: true,
            theme: "system".into(),
        }
    }
}

fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("SwitchDesk").join("config.json")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}
