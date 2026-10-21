//! Один экземпляр программы на сеанс и способ попросить его показать окно.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
};

use crate::wide;

const ALREADY_EXISTS: u32 = 183;

/// Занимает «замок» экземпляра. `false` — программа уже запущена.
/// Дескриптор намеренно не закрывается: замок живёт до выхода процесса.
pub fn acquire() -> bool {
    let name = wide("Local\\Switchdesk.Instance");
    unsafe {
        let h = CreateMutexW(ptr::null(), 0, name.as_ptr());
        !h.is_null() && GetLastError() != ALREADY_EXISTS
    }
}

fn show_event() -> HANDLE {
    let name = wide("Local\\Switchdesk.Show");
    unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) }
}

/// Просит уже запущенный экземпляр показать окно.
pub fn request_show() {
    let h = show_event();
    if !h.is_null() {
        unsafe {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

/// Блокирует поток и вызывает `f` при каждом запросе показать окно.
pub fn wait_show_requests(f: impl Fn()) {
    let h = show_event();
    if h.is_null() {
        return;
    }
    while unsafe { WaitForSingleObject(h, INFINITE) } == 0 {
        f();
    }
}
