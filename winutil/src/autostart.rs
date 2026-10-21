//! Автозапуск текущего пользователя (HKCU\...\Run).

use std::ptr;

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::wide;

const SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE: &str = "Switchdesk";

fn open(access: u32) -> Option<HKEY> {
    let key = wide(SUBKEY);
    let mut h: HKEY = ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            access,
            ptr::null(),
            &mut h,
            ptr::null_mut(),
        )
    };
    (rc == 0).then_some(h)
}

pub fn is_enabled() -> bool {
    let Some(h) = open(KEY_QUERY_VALUE) else { return false };
    let name = wide(VALUE);
    let rc = unsafe {
        RegQueryValueExW(h, name.as_ptr(), ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
    };
    unsafe { RegCloseKey(h) };
    rc == 0
}

/// Включает автозапуск с командной строкой `command` или выключает его.
pub fn set(enabled: bool, command: &str) -> bool {
    let Some(h) = open(KEY_SET_VALUE) else { return false };
    let name = wide(VALUE);
    let ok = unsafe {
        if enabled {
            let data = wide(command);
            RegSetValueExW(h, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) == 0
        } else {
            // 2 = ERROR_FILE_NOT_FOUND: значения и так нет
            matches!(RegDeleteValueW(h, name.as_ptr()), 0 | 2)
        }
    };
    unsafe { RegCloseKey(h) };
    ok
}
