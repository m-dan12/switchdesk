//! Менеджер сессий: к компьютеру может быть подключено сколько угодно клиентов сразу,
//! все пользуются им совместно. У каждой сессии свой учёт зажатого, поэтому при отключении
//! отпускается только то, что держал именно этот клиент. Не зависит от ОС — покрыт тестами.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use switchdesk_proto::{Button, ClientMsg, HostMsg};

use super::held::{Decision, HeldState};

/// Побочные эффекты на принимающем компьютере.
pub trait Backend: Send + Sync {
    /// Внедрить событие ввода.
    fn apply(&self, msg: &ClientMsg);
    /// Отпустить клавиши и кнопки.
    fn release(&self, keys: &[(u16, bool)], buttons: &[Button]);
}

/// Канал связи с клиентом.
pub trait Peer: Send + Sync {
    fn send(&self, msg: &HostMsg);
    fn close(&self);
}

pub struct Session {
    pub id: u64,
    pub name: String,
    peer: Arc<dyn Peer>,
    held: Mutex<HeldState>,
}

pub struct Manager<B: Backend> {
    backend: B,
    sessions: Mutex<Vec<Arc<Session>>>,
    next_id: AtomicU64,
}

impl<B: Backend> Manager<B> {
    pub fn new(backend: B) -> Self {
        Self { backend, sessions: Mutex::new(Vec::new()), next_id: AtomicU64::new(1) }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn add(&self, name: String, peer: Arc<dyn Peer>) -> Arc<Session> {
        let s = Arc::new(Session {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            name,
            peer,
            held: Mutex::new(HeldState::default()),
        });
        self.sessions.lock().unwrap().push(s.clone());
        s
    }

    /// Убирает сессию и отпускает то, что она держала.
    pub fn remove(&self, s: &Session) {
        self.sessions.lock().unwrap().retain(|x| x.id != s.id);
        self.release_held(s);
    }

    /// Внедряет событие этой сессии. Замок менеджера при этом не удерживается:
    /// сессии не мешают друг другу.
    pub fn input(&self, s: &Session, msg: &ClientMsg) {
        let decision = s.held.lock().unwrap().decide(msg);
        if decision == Decision::Inject {
            self.backend.apply(msg);
        }
    }

    /// Отпускает всё, что держит сессия (клиент сам попросил или отключился).
    pub fn release_held(&self, s: &Session) {
        let (keys, buttons) = s.held.lock().unwrap().drain();
        if !keys.is_empty() || !buttons.is_empty() {
            self.backend.release(&keys, &buttons);
        }
    }

    fn snapshot(&self) -> Vec<Arc<Session>> {
        self.sessions.lock().unwrap().clone()
    }

    /// Отправляет сообщение всем, кроме `except` (если задан).
    pub fn send_to_others(&self, except: Option<u64>, msg: &HostMsg) {
        for s in self.snapshot() {
            if Some(s.id) != except {
                s.peer.send(msg);
            }
        }
    }

    pub fn has_active(&self) -> bool {
        !self.sessions.lock().unwrap().is_empty()
    }

    /// Имена тех, кто сейчас подключён, по порядку подключения.
    pub fn names(&self) -> Vec<String> {
        self.sessions.lock().unwrap().iter().map(|s| s.name.clone()).collect()
    }

    /// Закрывает все сессии и отпускает всё зажатое.
    pub fn reset(&self) {
        for s in std::mem::take(&mut *self.sessions.lock().unwrap()) {
            s.peer.close();
            self.release_held(&s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    type Released = (Vec<(u16, bool)>, Vec<Button>);

    #[derive(Default)]
    struct MockBackend {
        applied: Mutex<Vec<ClientMsg>>,
        released: Mutex<Vec<Released>>,
    }
    impl Backend for Arc<MockBackend> {
        fn apply(&self, msg: &ClientMsg) {
            self.applied.lock().unwrap().push(msg.clone());
        }
        fn release(&self, keys: &[(u16, bool)], buttons: &[Button]) {
            self.released.lock().unwrap().push((keys.to_vec(), buttons.to_vec()));
        }
    }

    #[derive(Default)]
    struct MockPeer {
        sent: Mutex<Vec<HostMsg>>,
        closed: AtomicUsize,
    }
    impl Peer for MockPeer {
        fn send(&self, msg: &HostMsg) {
            self.sent.lock().unwrap().push(msg.clone());
        }
        fn close(&self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn setup() -> (Manager<Arc<MockBackend>>, Arc<MockBackend>) {
        let b = Arc::new(MockBackend::default());
        (Manager::new(b.clone()), b)
    }

    fn key(scan: u16, up: bool) -> ClientMsg {
        ClientMsg::Key { scan, extended: false, up }
    }

    #[test]
    fn several_clients_work_at_once_without_kicking() {
        let (m, b) = setup();
        let (p1, p2) = (Arc::new(MockPeer::default()), Arc::new(MockPeer::default()));
        let s1 = m.add("Даниил".into(), p1.clone());
        let s2 = m.add("Коллега".into(), p2.clone());

        assert_eq!(m.names(), vec!["Даниил".to_string(), "Коллега".to_string()]);
        assert!(p1.sent.lock().unwrap().is_empty() && p2.sent.lock().unwrap().is_empty());
        assert_eq!(p1.closed.load(Ordering::SeqCst), 0);

        m.input(&s1, &ClientMsg::MouseMove { dx: 1, dy: 2 });
        m.input(&s2, &key(0x1E, false));
        assert_eq!(b.applied.lock().unwrap().len(), 2, "ввод обоих проходит");
    }

    #[test]
    fn disconnect_releases_only_own_keys() {
        let (m, b) = setup();
        let s1 = m.add("A".into(), Arc::new(MockPeer::default()));
        let s2 = m.add("B".into(), Arc::new(MockPeer::default()));
        m.input(&s1, &key(0x1D, false)); // Ctrl у A
        m.input(&s2, &key(0x2A, false)); // Shift у B

        m.remove(&s1);
        let rel = b.released.lock().unwrap().clone();
        assert_eq!(rel, vec![(vec![(0x1D, false)], vec![])], "отпущен только Ctrl от A");
        assert_eq!(m.names(), vec!["B".to_string()]);

        // повторное удаление ничего не отпускает второй раз
        m.remove(&s1);
        assert_eq!(b.released.lock().unwrap().len(), 1);
    }

    #[test]
    fn release_up_for_unpressed_key_is_not_injected() {
        let (m, b) = setup();
        let s = m.add("A".into(), Arc::new(MockPeer::default()));
        m.input(&s, &key(0x1D, true));
        assert!(b.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn broadcast_skips_the_sender() {
        let (m, _b) = setup();
        let (p1, p2, p3) = (Arc::new(MockPeer::default()), Arc::new(MockPeer::default()), Arc::new(MockPeer::default()));
        let s1 = m.add("A".into(), p1.clone());
        m.add("B".into(), p2.clone());
        m.add("C".into(), p3.clone());
        m.send_to_others(Some(s1.id), &HostMsg::Clipboard("x".into()));
        assert!(p1.sent.lock().unwrap().is_empty());
        assert_eq!(p2.sent.lock().unwrap().len(), 1);
        assert_eq!(p3.sent.lock().unwrap().len(), 1);
        m.send_to_others(None, &HostMsg::Clipboard("y".into()));
        assert_eq!(p1.sent.lock().unwrap().len(), 1);
    }

    #[test]
    fn reset_closes_everyone_and_releases_everything() {
        let (m, b) = setup();
        let p = Arc::new(MockPeer::default());
        let s = m.add("A".into(), p.clone());
        m.input(&s, &key(0x1D, false));
        m.reset();
        assert!(!m.has_active());
        assert_eq!(p.closed.load(Ordering::SeqCst), 1);
        assert_eq!(b.released.lock().unwrap().len(), 1);
    }
}
