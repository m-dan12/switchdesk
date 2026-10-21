//! Сохранение и запуск установщика.

use std::io;
use std::path::{Path, PathBuf};

const DIR_PREFIX: &str = "switchdesk-update-";

/// Убирает папки прошлых обновлений (установщик к тому времени давно отработал).
fn cleanup_old() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(DIR_PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Кладёт установщик во временную папку и возвращает путь.
pub fn store(bytes: &[u8]) -> io::Result<PathBuf> {
    cleanup_old();
    let dir = std::env::temp_dir().join(format!("{DIR_PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(crate::INSTALLER_ASSET);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Подходит ли строка для вставки в командную строку cmd.exe внутри кавычек.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn cmd_safe(s: &str) -> bool {
    !s.chars().any(|c| matches!(c, '"' | '%' | '^' | '!' | '\r' | '\n' | '\0'))
}

/// Командная строка для `cmd /C`: тихая установка, затем (по желанию) перезапуск программы.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn build_command_line(installer: &Path, relaunch: Option<&(PathBuf, Vec<String>)>) -> Option<String> {
    let inst = installer.display().to_string();
    if !cmd_safe(&inst) {
        return None;
    }
    let mut line = format!(
        "start \"\" /wait \"{inst}\" /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS"
    );
    if let Some((exe, args)) = relaunch {
        let exe = exe.display().to_string();
        // Небезопасные для cmd символы в путях/аргументах: лучше не перезапускаться,
        // чем исполнить что-то лишнее.
        if cmd_safe(&exe) && args.iter().all(|a| cmd_safe(a)) {
            line.push_str(&format!(" & start \"\" \"{exe}\""));
            for a in args {
                line.push_str(&format!(" \"{a}\""));
            }
        }
    }
    Some(line)
}

/// Запускает установщик отдельным процессом и сразу возвращается: вызывающий должен
/// завершиться, чтобы установщик мог заменить файлы. `relaunch` — что запустить после.
#[cfg(windows)]
pub fn run_installer(installer: &Path, relaunch: Option<(PathBuf, Vec<String>)>) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let line = build_command_line(installer, relaunch.as_ref())
        .ok_or_else(|| io::Error::other("недопустимые символы в пути установщика"))?;
    std::process::Command::new("cmd.exe")
        .raw_arg(format!("/D /S /C \"{line}\""))
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
pub fn run_installer(_installer: &Path, _relaunch: Option<(PathBuf, Vec<String>)>) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "обновление работает только в Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_with_relaunch() {
        let rel = (PathBuf::from(r"C:\Program Files\S\c.exe"), vec!["--host".into(), "a b:1".into(), "--no-auto-update".into()]);
        let line = build_command_line(Path::new(r"C:\T\switchdesk-setup.exe"), Some(&rel)).unwrap();
        assert_eq!(
            line,
            r#"start "" /wait "C:\T\switchdesk-setup.exe" /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS & start "" "C:\Program Files\S\c.exe" "--host" "a b:1" "--no-auto-update""#
        );
    }

    #[test]
    fn unsafe_args_skip_relaunch_but_still_install() {
        let rel = (PathBuf::from("c.exe"), vec!["x\" & calc".into()]);
        let line = build_command_line(Path::new("i.exe"), Some(&rel)).unwrap();
        assert!(!line.contains("calc") && !line.contains("& start"));
        assert!(build_command_line(Path::new("i%PATH%.exe"), None).is_none());
    }
}
