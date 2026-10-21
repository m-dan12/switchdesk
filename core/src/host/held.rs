//! Учёт зажатых клавиш/кнопок одной сессии и решение, внедрять ли событие.
//! Не зависит от ОС — покрыт тестами.

use std::collections::HashSet;

use switchdesk_proto::{Button, ClientMsg};

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Внедрить событие.
    Inject,
    /// Событие бессмысленно (например, отпускание не зажатой клавиши).
    Ignore,
}

#[derive(Default)]
pub struct HeldState {
    keys: HashSet<(u16, bool)>,
    buttons: HashSet<Button>,
}

impl HeldState {
    /// Решает судьбу сообщения и обновляет состояние.
    pub fn decide(&mut self, msg: &ClientMsg) -> Decision {
        match *msg {
            ClientMsg::MouseMove { .. } | ClientMsg::Wheel { .. } => Decision::Inject,
            ClientMsg::MouseButton { button, down: true } => {
                self.buttons.insert(button);
                Decision::Inject
            }
            ClientMsg::MouseButton { button, down: false } => {
                if self.buttons.remove(&button) {
                    Decision::Inject
                } else {
                    Decision::Ignore
                }
            }
            ClientMsg::Key { scan, extended, up: false } => {
                self.keys.insert((scan, extended));
                Decision::Inject
            }
            ClientMsg::Key { scan, extended, up: true } => {
                if self.keys.remove(&(scan, extended)) {
                    Decision::Inject
                } else {
                    Decision::Ignore
                }
            }
            _ => Decision::Ignore,
        }
    }

    /// Забирает всё зажатое (для отпускания при отключении).
    pub fn drain(&mut self) -> (Vec<(u16, bool)>, Vec<Button>) {
        (self.keys.drain().collect(), self.buttons.drain().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(scan: u16, up: bool) -> ClientMsg {
        ClientMsg::Key { scan, extended: false, up }
    }

    #[test]
    fn up_without_down_is_ignored() {
        let mut h = HeldState::default();
        assert_eq!(h.decide(&key(0x1D, true)), Decision::Ignore);
        assert_eq!(h.decide(&key(0x1D, false)), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true)), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true)), Decision::Ignore);
    }

    #[test]
    fn movement_and_wheel_always_pass() {
        let mut h = HeldState::default();
        assert_eq!(h.decide(&ClientMsg::MouseMove { dx: 1, dy: 1 }), Decision::Inject);
        assert_eq!(h.decide(&ClientMsg::Wheel { delta: 120, horizontal: false }), Decision::Inject);
    }

    #[test]
    fn extended_flag_distinguishes_keys() {
        let mut h = HeldState::default();
        let ext = ClientMsg::Key { scan: 0x1D, extended: true, up: false };
        assert_eq!(h.decide(&ext), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true)), Decision::Ignore);
    }

    #[test]
    fn drain_returns_everything_held() {
        let mut h = HeldState::default();
        h.decide(&key(1, false));
        h.decide(&key(2, false));
        h.decide(&ClientMsg::MouseButton { button: Button::Left, down: true });
        let (mut keys, buttons) = h.drain();
        keys.sort();
        assert_eq!(keys, vec![(1, false), (2, false)]);
        assert_eq!(buttons, vec![Button::Left]);
        let (k2, b2) = h.drain();
        assert!(k2.is_empty() && b2.is_empty());
    }

    #[test]
    fn control_messages_are_ignored() {
        let mut h = HeldState::default();
        assert_eq!(h.decide(&ClientMsg::Ping(1)), Decision::Ignore);
        assert_eq!(h.decide(&ClientMsg::ReleaseAll), Decision::Ignore);
    }
}
