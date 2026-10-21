//! Общие флаги обновления для kvm-host и kvm-client.

use std::process;

/// Флаги, которые снимаются из аргументов до разбора остальных.
#[derive(Default)]
pub struct Opts {
    pub version: bool,
    pub check: bool,
    pub update: bool,
    pub no_auto: bool,
}

impl Opts {
    /// Команда, после которой программа завершается (без обычного запуска).
    pub fn is_command(&self) -> bool {
        self.version || self.check || self.update
    }
}

/// Вынимает `--version`, `--check-update`, `--update`, `--no-auto-update`.
pub fn take_flags(args: &mut Vec<String>) -> Opts {
    let mut o = Opts::default();
    args.retain(|a| {
        let flag = match a.as_str() {
            "--version" | "-V" => &mut o.version,
            "--check-update" => &mut o.check,
            "--update" => &mut o.update,
            "--no-auto-update" => &mut o.no_auto,
            _ => return true,
        };
        *flag = true;
        false
    });
    o
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    process::exit(1);
}

/// Выполняет команды `--version`, `--check-update`, `--update` и завершает программу.
/// Без команды ничего не делает.
pub fn run(app: &str, o: &Opts) {
    if o.version {
        println!("{app} {}", crate::CURRENT);
        process::exit(0);
    }
    if o.check {
        match crate::check() {
            Ok(Some(r)) => println!("Доступна версия {} (у вас {}). Установить: {app} --update", r.version, crate::CURRENT),
            Ok(None) => println!("Установлена последняя версия ({}).", crate::CURRENT),
            Err(e) => fail(&e),
        }
        process::exit(0);
    }
    if o.update {
        match crate::check() {
            Ok(Some(r)) => {
                println!("Скачиваю версию {}…", r.version);
                let path = crate::download(&r).unwrap_or_else(|e| fail(&e));
                println!("Подпись проверена. Запускаю установщик.");
                crate::run_installer(&path, None).unwrap_or_else(|e| fail(&format!("не удалось запустить установщик: {e}")));
            }
            Ok(None) => println!("Установлена последняя версия ({}).", crate::CURRENT),
            Err(e) => fail(&e),
        }
        process::exit(0);
    }
}
