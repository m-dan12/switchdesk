//! Инъекция ввода на хосте (SendInput).

use std::mem::size_of;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

fn send(input: INPUT) {
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) {
    send(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    });
}

pub fn mouse_move(dx: i32, dy: i32) {
    mouse(MOUSEEVENTF_MOVE, dx.clamp(-4000, 4000), dy.clamp(-4000, 4000), 0);
}

pub fn button(b: u8, down: bool) {
    let (flags, data) = match (b, down) {
        (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (0, false) => (MOUSEEVENTF_LEFTUP, 0),
        (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (3, true) => (MOUSEEVENTF_XDOWN, 1),
        (3, false) => (MOUSEEVENTF_XUP, 1),
        (4, true) => (MOUSEEVENTF_XDOWN, 2),
        (4, false) => (MOUSEEVENTF_XUP, 2),
        _ => return,
    };
    mouse(flags, 0, 0, data);
}

pub fn wheel(v: i32, horizontal: bool) {
    let flags = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
    mouse(flags, 0, 0, v as u32);
}

pub fn key(sc: u16, ext: bool, down: bool) {
    let mut flags = KEYEVENTF_SCANCODE;
    if ext {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    send(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: sc, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    });
}
