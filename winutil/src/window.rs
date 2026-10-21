//! Невидимое окно для сообщений (WM_INPUT, WM_CLIPBOARDUPDATE) и цикл сообщений.

use std::ptr;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, HWND_MESSAGE,
    MSG, WNDCLASSW, WNDPROC,
};

use crate::wide;

/// Создаёт message-only окно. `None` при ошибке.
pub fn create_message_window(class_name: &str, proc: WNDPROC) -> Option<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let class = wide(class_name);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            hinst,
            ptr::null(),
        );
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    }
}

/// Крутит цикл сообщений текущего потока до WM_QUIT. Нужен и хукам, и окну.
pub fn run_message_loop() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
