//! switchdesk-client: перехватывает мышь и клавиатуру и по горячей клавише (Ctrl+Alt+F12)
//! перенаправляет их на хост; повторное нажатие возвращает ввод на этот компьютер.

#[cfg(any(windows, switchdesk_check))]
mod net;
#[cfg(any(windows, switchdesk_check))]
mod win;

pub struct Config {
    /// Адрес хоста: имя MagicDNS или IP Tailscale + порт.
    pub host: String,
    /// Имя, под которым вы видны хосту и вытесненному пользователю.
    pub name: String,
}

const USAGE: &str = "\
switchdesk-client — переключатель мыши и клавиатуры

  switchdesk-client --host АДРЕС:ПОРТ [--name ИМЯ]

  --host   адрес хоста, например server.tailnet.ts.net:7878 или 100.x.y.z:7878
  --name   ваше имя (по умолчанию — имя компьютера)

Горячая клавиша: Ctrl+Alt+F12 — переключить ввод на хост / обратно.
Она обрабатывается локально и работает даже при обрыве сети.";

pub fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn parse_args() -> Config {
    let mut host = None;
    let mut name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "client".into());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--host" => host = Some(args.next().unwrap_or_else(|| die("--host требует значение"))),
            "--name" => name = args.next().unwrap_or_else(|| die("--name требует значение")),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => die(&format!("неизвестный аргумент: {other}\n\n{USAGE}")),
        }
    }
    let host = host.unwrap_or_else(|| die(&format!("не указан --host\n\n{USAGE}")));
    Config { host, name }
}

fn main() {
    let cfg = parse_args();
    #[cfg(any(windows, switchdesk_check))]
    win::run(cfg);
    #[cfg(not(any(windows, switchdesk_check)))]
    {
        let _ = cfg;
        die("switchdesk-client работает только на Windows");
    }
}
