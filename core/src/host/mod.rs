//! Принимающая сторона: учёт зажатых клавиш, менеджер сессий, сеть.

pub mod held;
pub mod session;

#[cfg(any(windows, switchdesk_check))]
pub mod net;
