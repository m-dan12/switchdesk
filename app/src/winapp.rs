//! Мелкие Windows-функции приложения: один экземпляр, показ/скрытие окна, автозапуск.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
};

/// Не даёт Windows 11 переводить скрытое окно в режим эффективности (EcoQoS): в нём потоки
/// просыпаются с задержкой, и мышь через сеть подлагивает. Заодно чуть повышает приоритет процесса.
pub fn disable_throttling() {
    use windows::Win32::System::Threading::*;
    unsafe {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        let _ = SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
    }
}

pub const WINDOW_TITLE: &str = "SwitchDesk";
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("SwitchDesk");

/// true, если это первый экземпляр. Иначе показывает окно уже запущенного.
pub fn claim_single_instance() -> bool {
    unsafe {
        match CreateMutexW(None, true, w!(r"Local\SwitchDeskSingleInstance")) {
            Ok(h) => {
                let exists = GetLastError() == ERROR_ALREADY_EXISTS;
                let _ = h; // дескриптор живёт до конца процесса
                if exists {
                    show_window();
                }
                !exists
            }
            Err(_) => true,
        }
    }
}

fn find_window() -> Option<HWND> {
    let title: Vec<u16> = WINDOW_TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())).ok().filter(|h| !h.is_invalid()) }
}

pub fn show_window() {
    if let Some(h) = find_window() {
        unsafe {
            let _ = ShowWindow(h, SW_SHOW);
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
    }
}

pub fn hide_window() {
    if let Some(h) = find_window() {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

pub fn autostart_enabled() -> bool {
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None).is_ok()
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd = format!("\"{}\" --minimized", exe.display());
            let data: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(data.as_ptr() as *const _),
                (data.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

// ----- запуск с правами администратора -----
//
// Окна с повышенными правами (диспетчер задач, игры с защитой) не принимают ввод от обычного
// процесса и не показывают ему перехватываемый ввод (UIPI). Поэтому SwitchDesk можно запускать
// через задачу планировщика с наивысшими правами: UAC спрашивает один раз, при создании задачи.

const TASK: &str = "SwitchDesk";

fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}

pub fn is_elevated() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = Default::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok.is_ok() && e.TokenIsElevated != 0
    }
}

pub fn elevated_task_exists() -> bool {
    hidden(std::process::Command::new("schtasks").args(["/Query", "/TN", TASK]))
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Запускает задачу (обычный пользователь может её запустить, если сам её создал).
pub fn run_elevated_task() -> bool {
    hidden(std::process::Command::new("schtasks").args(["/Run", "/TN", TASK]))
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Создаёт или удаляет задачу; нужен запрос UAC. true — действие выполнено.
pub fn set_elevated(on: bool) -> bool {
    use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let script = if on {
        let Ok(exe) = std::env::current_exe() else { return false };
        let exe = exe.display().to_string().replace('\'', "''");
        format!(
            "$u = [Security.Principal.WindowsIdentity]::GetCurrent().Name;              $a = New-ScheduledTaskAction -Execute '{exe}' -Argument '--minimized';              $t = New-ScheduledTaskTrigger -AtLogOn -User $u;              $p = New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Highest;              $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries                -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew;              Register-ScheduledTask -TaskName {TASK} -Action $a -Trigger $t -Principal $p -Settings $s -Force"
        )
    } else {
        format!("Unregister-ScheduledTask -TaskName {TASK} -Confirm:$false")
    };
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let args = format!(
        "-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(utf16)
    );
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (verb, file, params) = (wide("runas"), wide("powershell.exe"), wide(&args));
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if ShellExecuteExW(&mut info).is_err() || info.hProcess.is_invalid() {
            return false; // отказ в UAC
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = windows::Win32::Foundation::CloseHandle(info.hProcess);
    }
    elevated_task_exists() == on
}

/// Перезапускает программу через задачу с правами администратора и завершает этот процесс.
pub fn restart_elevated() -> ! {
    capture_release();
    let _ = hidden(std::process::Command::new("cmd").args([
        "/c",
        &format!("ping -n 3 127.0.0.1 >nul & schtasks /Run /TN {TASK}"),
    ]))
    .spawn();
    std::process::exit(0)
}

fn capture_release() {
    crate::capture::set_sink(None);
}
