//! Локальное управление запущенным экземпляром: `switchdesk.exe --switch <устройство|local>`.
//! Команда уходит UDP-пакетом на 127.0.0.1, так её можно привязать к кнопке стороннего меню.

use crate::capture;
use crate::client::{Client, Status};
use crate::config::Config;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const PORT: u16 = 52801;

/// Отправить команду уже запущенному экземпляру.
pub fn send(target: &str) {
    if let Ok(s) = UdpSocket::bind("127.0.0.1:0") {
        let _ = s.send_to(target.as_bytes(), ("127.0.0.1", PORT));
    }
}

/// Слушать команды: имя устройства — подключиться и передать на него ввод, `local` — вернуть ввод себе.
pub fn listen(client: Client) {
    let Ok(sock) = UdpSocket::bind(("127.0.0.1", PORT)) else { return };
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok((n, _)) = sock.recv_from(&mut buf) {
            let target = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            // Если этим ПК управляют, решение принимает управляющий, а не мы.
            if !crate::host::forward_switch(&target) {
                apply(&client, &target);
            }
        }
    });
}

pub fn apply(client: &Client, target: &str) {
    if target.eq_ignore_ascii_case("local") {
        if capture::is_active() {
            capture::toggle();
        }
        return;
    }
    let cfg = Config::load();
    if target.eq_ignore_ascii_case(&cfg.pc_name) {
        // Выбран сам этот компьютер: просто вернуть ввод себе и отключиться.
        if capture::is_active() {
            capture::toggle();
        }
        client.disconnect();
        return;
    }
    let Some(dev) = cfg.devices.iter().find(|d| d.name.eq_ignore_ascii_case(target)).cloned() else { return };
    if capture::is_active() {
        capture::toggle();
    }
    let same = matches!(client.status(), Status::Connected(_)) && client.target() == Some(dev.name.clone());
    if !same {
        client.connect(dev.clone(), cfg.pc_name.clone());
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(6) {
        if matches!(client.status(), Status::Connected(_)) {
            if !capture::is_active() {
                capture::toggle();
            }
            return;
        }
        if matches!(client.status(), Status::Error(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
