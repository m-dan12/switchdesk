//! switchdesk-host: принимает ввод от одного активного клиента и внедряет его на этом компьютере.
#![cfg_attr(not(any(windows, switchdesk_check)), allow(dead_code))]

mod held;
mod session;
#[cfg(any(windows, switchdesk_check))]
mod win;

pub struct Config {
    /// Адрес прослушивания. Для Tailscale — ваш 100.x.y.z:порт.
    pub bind: String,
    /// Сколько мс после физического ввода на хосте игнорировать удалённый (0 — не игнорировать).
    pub grace_ms: u64,
}

const USAGE: &str = "\
switchdesk-host — приёмник ввода

  switchdesk-host [--bind АДРЕС:ПОРТ] [--grace-ms N]

  --bind       адрес прослушивания (по умолчанию 127.0.0.1:7878).
               Для работы по Tailscale укажите адрес вида 100.x.y.z:7878.
  --grace-ms   пауза удалённого ввода после физического (по умолчанию 1500, 0 — выкл.)";

fn parse_args(args: Vec<String>) -> Config {
    let mut cfg = Config { bind: "127.0.0.1:7878".into(), grace_ms: 1500 };
    let mut args = args.into_iter();
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
                println!("{USAGE}\n\n{}", switchdesk_update::cli::USAGE);
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
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let upd = switchdesk_update::cli::take_flags(&mut args);
    if upd.is_command() {
        switchdesk_update::cli::run("switchdesk-host", &upd, &args);
    }
    let cfg = parse_args(args.clone());
    switchdesk_update::cli::run("switchdesk-host", &upd, &args);
    #[cfg(any(windows, switchdesk_check))]
    win::run(cfg);
    #[cfg(not(any(windows, switchdesk_check)))]
    {
        let _ = cfg;
        die("switchdesk-host работает только на Windows");
    }
}
