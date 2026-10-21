//! Менеджер сессий: на хосте активен не более одного клиента.
//! Новый клиент вытесняет предыдущего. Не зависит от ОС — покрыт тестами.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use switchdesk_proto::HostMsg;

/// Побочные эффекты на хосте.
pub trait Backend: Send + Sync {
    /// Отпустить все зажатые клавиши и кнопки.
    fn release_all(&self);
    /// Очистить буфер обмена (чтобы следующий пользователь не увидел чужое).
    fn clear_clipboard(&self);
}

/// Канал связи с клиентом.
pub trait Peer: Send + Sync {
    fn send(&self, msg: &HostMsg);
    fn close(&self);
}

struct Active {
    id: u64,
    name: String,
    peer: Arc<dyn Peer>,
}

pub struct Manager<B: Backend> {
    backend: B,
    active: Mutex<Option<Active>>,
    next_id: AtomicU64,
}

impl<B: Backend> Manager<B> {
    pub fn new(backend: B) -> Self {
        Self { backend, active: Mutex::new(None), next_id: AtomicU64::new(1) }
    }

    /// Делает клиента активным. Предыдущий получает `Kicked`, соединение с ним
    /// закрывается, а хост приводится в чистое состояние — всё под одним замком,
    /// поэтому новый клиент не успевает ничего внедрить до очистки.
    pub fn activate(&self, name: String, peer: Arc<dyn Peer>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.active.lock().unwrap();
        if let Some(old) = guard.take() {
            crate::state::log(format!("«{}» вытеснен: управление взял «{}»", old.name, name));
            old.peer.send(&HostMsg::Kicked { by: name.clone() });
            old.peer.close();
            self.backend.release_all();
            self.backend.clear_clipboard();
        }
        crate::state::log(format!("«{}» управляет этим компьютером", name));
        *guard = Some(Active { id, name, peer });
        id
    }

    /// Завершает сессию, если она всё ещё активна (после кика — ничего не делает).
    pub fn release(&self, id: u64) {
        let mut guard = self.active.lock().unwrap();
        if guard.as_ref().map(|a| a.id) == Some(id) {
            let old = guard.take().unwrap();
            crate::state::log(format!("«{}» отключился", old.name));
            self.backend.release_all();
            self.backend.clear_clipboard();
        }
    }

    /// Выполняет `f`, только если сессия `id` активна; замок удерживается на время вызова,
    /// поэтому внедрение ввода не пересекается с киком и очисткой.
    pub fn run_if_active<R>(&self, id: u64, f: impl FnOnce(&B) -> R) -> Option<R> {
        let guard = self.active.lock().unwrap();
        match guard.as_ref() {
            Some(a) if a.id == id => Some(f(&self.backend)),
            _ => None,
        }
    }

    /// Отправляет сообщение активному клиенту, если он есть.
    pub fn send_active(&self, msg: &HostMsg) {
        let peer = self.active.lock().unwrap().as_ref().map(|a| a.peer.clone());
        if let Some(p) = peer {
            p.send(msg);
        }
    }

    pub fn has_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// Имя того, кто сейчас управляет этим компьютером.
    pub fn active_name(&self) -> Option<String> {
        self.active.lock().unwrap().as_ref().map(|a| a.name.clone())
    }

    /// Закрывает активную сессию (если есть) и приводит хост в чистое состояние.
    pub fn reset(&self) {
        let old = self.active.lock().unwrap().take();
        if let Some(old) = old {
            old.peer.close();
        }
        self.backend.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct MockBackend {
        releases: AtomicUsize,
        clears: AtomicUsize,
    }
    impl Backend for Arc<MockBackend> {
        fn release_all(&self) {
            self.releases.fetch_add(1, Ordering::SeqCst);
        }
        fn clear_clipboard(&self) {
            self.clears.fetch_add(1, Ordering::SeqCst);
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

    #[test]
    fn second_client_kicks_first_and_cleans_host() {
        let (m, b) = setup();
        let p1 = Arc::new(MockPeer::default());
        let p2 = Arc::new(MockPeer::default());
        let id1 = m.activate("Даниил".into(), p1.clone());
        let id2 = m.activate("Коллега".into(), p2.clone());

        assert_eq!(p1.sent.lock().unwrap().as_slice(), &[HostMsg::Kicked { by: "Коллега".into() }]);
        assert_eq!(p1.closed.load(Ordering::SeqCst), 1);
        assert!(p2.sent.lock().unwrap().is_empty());
        assert_eq!(b.releases.load(Ordering::SeqCst), 1);
        assert_eq!(b.clears.load(Ordering::SeqCst), 1);

        assert!(m.run_if_active(id1, |_| ()).is_none(), "вытесненный не должен вводить");
        assert!(m.run_if_active(id2, |_| ()).is_some());
    }

    #[test]
    fn late_release_of_kicked_session_is_noop() {
        let (m, b) = setup();
        let id1 = m.activate("A".into(), Arc::new(MockPeer::default()));
        let _id2 = m.activate("B".into(), Arc::new(MockPeer::default()));
        m.release(id1);
        // чистка была ровно одна — при кике; позднее release старого id её не повторяет
        assert_eq!(b.releases.load(Ordering::SeqCst), 1);
        assert_eq!(b.clears.load(Ordering::SeqCst), 1);
        assert!(m.has_active());
    }

    #[test]
    fn release_active_cleans_and_frees_host() {
        let (m, b) = setup();
        let id = m.activate("A".into(), Arc::new(MockPeer::default()));
        m.release(id);
        assert!(!m.has_active());
        assert_eq!(b.releases.load(Ordering::SeqCst), 1);
        assert_eq!(b.clears.load(Ordering::SeqCst), 1);
        // повторный release безвреден
        m.release(id);
        assert_eq!(b.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn send_active_reaches_only_current_peer() {
        let (m, _b) = setup();
        let p1 = Arc::new(MockPeer::default());
        let p2 = Arc::new(MockPeer::default());
        m.activate("A".into(), p1.clone());
        m.activate("B".into(), p2.clone());
        m.send_active(&HostMsg::Clipboard("x".into()));
        assert_eq!(p1.sent.lock().unwrap().len(), 1, "только Kicked");
        assert_eq!(p2.sent.lock().unwrap().as_slice(), &[HostMsg::Clipboard("x".into())]);
    }
}
