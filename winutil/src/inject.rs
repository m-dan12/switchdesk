//! Синтетический ввод через SendInput.

use std::mem::size_of;

use kvm_proto::Button;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT,
};

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

fn send(input: INPUT) {
    unsafe {
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: u32, data: u32, dx: i32, dy: i32) {
    send(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    });
}

pub fn mouse_move(dx: i32, dy: i32) {
    mouse(MOUSEEVENTF_MOVE, 0, dx, dy);
}

pub fn mouse_button(button: Button, down: bool) {
    let (flags, data) = match (button, down) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (Button::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (Button::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (Button::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    };
    mouse(flags, data, 0, 0);
}

pub fn wheel(delta: i32, horizontal: bool) {
    let flags = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
    mouse(flags, delta as u32, 0, 0);
}

fn keybd(vk: u16, scan: u16, flags: u32) {
    send(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    });
}

/// Клавиша по сканкоду (не зависит от раскладки отправителя).
pub fn key(scan: u16, extended: bool, up: bool) {
    if scan == 0 {
        return;
    }
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    keybd(0, scan, flags);
}

/// Отпускает клавишу по виртуальному коду (для сброса модификаторов).
pub fn key_up_vk(vk: u16) {
    keybd(vk, 0, KEYEVENTF_KEYUP);
}
