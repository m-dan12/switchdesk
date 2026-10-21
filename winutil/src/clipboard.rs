//! Текст в буфере обмена (CF_UNICODETEXT).

use std::{ptr, thread, time::Duration};

use kvm_proto::MAX_CLIPBOARD_BYTES;
use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

use crate::wide;

const CF_UNICODETEXT: u32 = 13;

/// Буфер может быть занят другим процессом: несколько попыток с короткой паузой.
fn open(owner: HWND) -> bool {
    for _ in 0..5 {
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Менеджеры паролей помечают скопированные секреты этим форматом — их не синхронизируем.
pub fn is_excluded() -> bool {
    let name = wide("ExcludeClipboardContentFromMonitorProcessing");
    let fmt = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    fmt != 0 && unsafe { IsClipboardFormatAvailable(fmt) } != 0
}

/// Текст из буфера. `None`, если там не текст, он пуст или слишком большой.
pub fn get_text() -> Option<String> {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return None;
    }
    if !open(ptr::null_mut()) {
        return None;
    }
    let mut result = None;
    unsafe {
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let max_units = MAX_CLIPBOARD_BYTES / 2;
                let mut len = 0usize;
                while len <= max_units && *p.add(len) != 0 {
                    len += 1;
                }
                if len > 0 && len <= max_units {
                    result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                }
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
    }
    result
}

/// Кладёт в буфер DWORD-значение под именованным форматом (служебные флаги).
unsafe fn set_dword_format(name: &str, value: u32) {
    let w = wide(name);
    let fmt = RegisterClipboardFormatW(w.as_ptr());
    if fmt == 0 {
        return;
    }
    let h = GlobalAlloc(GMEM_MOVEABLE, 4);
    if h.is_null() {
        return;
    }
    let p = GlobalLock(h) as *mut u32;
    if p.is_null() {
        GlobalFree(h);
        return;
    }
    *p = value;
    GlobalUnlock(h);
    if SetClipboardData(fmt, h).is_null() {
        GlobalFree(h);
    }
}

/// Записывает текст. `private = true` запрещает попадание в историю буфера (Win+V)
/// и облачный буфер — нужно на общем хосте, чтобы чужой текст не оседал в истории.
pub fn set_text(text: &str, private: bool) -> bool {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * 2;
    if !open(ptr::null_mut()) {
        return false;
    }
    let mut ok = false;
    unsafe {
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                GlobalUnlock(h);
                if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                    GlobalFree(h);
                } else {
                    ok = true;
                }
            } else {
                GlobalFree(h);
            }
        }
        if ok && private {
            set_dword_format("CanIncludeInClipboardHistory", 0);
            set_dword_format("CanUploadToCloudClipboard", 0);
        }
        CloseClipboard();
    }
    ok
}

/// Полностью очищает буфер обмена.
pub fn clear() {
    if open(ptr::null_mut()) {
        unsafe {
            EmptyClipboard();
            CloseClipboard();
        }
    }
}
