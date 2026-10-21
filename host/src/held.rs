//! Учёт зажатых клавиш/кнопок на хосте и решение, внедрять ли событие.
//! Не зависит от ОС — покрыт тестами.

use std::collections::HashSet;

use kvm_proto::{Button, ClientMsg};

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Внедрить событие.
    Inject,
    /// Событие отброшено, потому что за хостом работают физически.
    Suppressed,
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
    ///
    /// При `suppressed` отбрасываются все «активные» события, но отпускание
    /// уже зажатого всё равно проходит — иначе на хосте залипнет клавиша.
    pub fn decide(&mut self, msg: &ClientMsg, suppressed: bool) -> Decision {
        match *msg {
            ClientMsg::MouseMove { .. } | ClientMsg::Wheel { .. } => {
                if suppressed {
                    Decision::Suppressed
                } else {
                    Decision::Inject
                }
            }
            ClientMsg::MouseButton { button, down: true } => {
                if suppressed {
                    Decision::Suppressed
                } else {
                    self.buttons.insert(button);
                    Decision::Inject
                }
            }
            ClientMsg::MouseButton { button, down: false } => {
                if self.buttons.remove(&button) {
                    Decision::Inject
                } else {
                    Decision::Ignore
                }
            }
            ClientMsg::Key { scan, extended, up: false } => {
                if suppressed {
                    Decision::Suppressed
                } else {
                    self.keys.insert((scan, extended));
                    Decision::Inject
                }
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

    /// Забирает всё зажатое (для `release_all`).
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
        assert_eq!(h.decide(&key(0x1D, true), false), Decision::Ignore);
        assert_eq!(h.decide(&key(0x1D, false), false), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true), false), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true), false), Decision::Ignore);
    }

    #[test]
    fn suppression_blocks_downs_but_not_releases() {
        let mut h = HeldState::default();
        assert_eq!(h.decide(&key(0x2A, false), false), Decision::Inject);
        // физический ввод на хосте: новые нажатия, движение и колесо отброшены…
        assert_eq!(h.decide(&key(0x1E, false), true), Decision::Suppressed);
        assert_eq!(h.decide(&ClientMsg::MouseMove { dx: 1, dy: 1 }, true), Decision::Suppressed);
        assert_eq!(
            h.decide(&ClientMsg::Wheel { delta: 120, horizontal: false }, true),
            Decision::Suppressed
        );
        // …но отпускание ранее зажатого проходит
        assert_eq!(h.decide(&key(0x2A, true), true), Decision::Inject);
    }

    #[test]
    fn extended_flag_distinguishes_keys() {
        let mut h = HeldState::default();
        let ext = ClientMsg::Key { scan: 0x1D, extended: true, up: false };
        assert_eq!(h.decide(&ext, false), Decision::Inject);
        assert_eq!(h.decide(&key(0x1D, true), false), Decision::Ignore);
    }

    #[test]
    fn drain_returns_everything_held() {
        let mut h = HeldState::default();
        h.decide(&key(1, false), false);
        h.decide(&key(2, false), false);
        h.decide(&ClientMsg::MouseButton { button: Button::Left, down: true }, false);
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
        assert_eq!(h.decide(&ClientMsg::Ping(1), false), Decision::Ignore);
        assert_eq!(h.decide(&ClientMsg::ReleaseAll, false), Decision::Ignore);
    }
}
