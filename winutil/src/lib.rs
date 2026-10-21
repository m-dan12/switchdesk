//! Общий Windows-код для host и client: буфер обмена, SendInput, служебные окна.
//!
//! На других ОС крейт пуст. Для проверки типов на Linux:
//! `RUSTFLAGS="--cfg switchdesk_check" cargo check --workspace`.
#![cfg(any(windows, switchdesk_check))]
#![allow(clippy::missing_safety_doc)]

pub mod clipboard;
pub mod inject;
pub mod window;

/// Строка в UTF-16 с нулевым терминатором.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Миллисекунды с момента запуска системы.
pub fn tick_ms() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}
