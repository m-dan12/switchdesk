//! kvm-host: принимает ввод от одного активного клиента и внедряет его на этом компьютере.
#![cfg_attr(not(any(windows, kvm_check)), allow(dead_code))]

mod held;
mod session;
#[cfg(any(windows, kvm_check))]
mod win;

pub struct Config {
    /// Адрес прослушивания. Для Tailscale — ваш 100.x.y.z:порт.
    pub bind: String,
    /// Сколько мс после физического ввода на хосте игнорировать удалённый (0 — не игнорировать).
    pub grace_ms: u64,
}

const USAGE: &str = "\
kvm-host — приёмник ввода

  kvm-host [--bind АДРЕС:ПОРТ] [--grace-ms N]

  --bind       адрес прослушивания (по умолчанию 127.0.0.1:7878).
               Для работы по Tailscale укажите адрес вида 100.x.y.z:7878.
  --grace-ms   пауза удалённого ввода после физического (по умолчанию 1500, 0 — выкл.)";

fn parse_args() -> Config {
    let mut cfg = Config { bind: "127.0.0.1:7878".into(), grace_ms: 1500 };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => cfg.bind = args.next().unwrap_or_else(|| die("--bind требует значение")),
            "--grace-ms" => {
                cfg.grace_ms = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--grace-ms требует число"))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => die(&format!("неизвестный аргумент: {other}\n\n{USAGE}")),
        }
    }
    cfg
}

pub fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn main() {
    let cfg = parse_args();
    #[cfg(any(windows, kvm_check))]
    win::run(cfg);
    #[cfg(not(any(windows, kvm_check)))]
    {
        let _ = cfg;
        die("kvm-host работает только на Windows");
    }
}
