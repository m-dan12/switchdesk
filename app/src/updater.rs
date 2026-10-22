//! Проверка и установка обновлений с GitHub Releases (подпись проверяет switchdesk-update).

use std::sync::{Arc, Mutex};
use switchdesk_update::Release;

#[derive(Clone, Debug)]
pub enum UpdState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    Error(String),
}

#[derive(Clone)]
pub struct Updater(Arc<Mutex<UpdState>>);

impl Updater {
    pub fn new() -> Updater {
        Updater(Arc::new(Mutex::new(UpdState::Idle)))
    }

    pub fn state(&self) -> UpdState {
        self.0.lock().unwrap().clone()
    }

    pub fn check(&self) {
        {
            let mut s = self.0.lock().unwrap();
            if matches!(*s, UpdState::Checking | UpdState::Downloading(_)) {
                return;
            }
            *s = UpdState::Checking;
        }
        let st = self.0.clone();
        std::thread::spawn(move || {
            let new = match switchdesk_update::check() {
                Ok(Some(r)) => UpdState::Available(r),
                Ok(None) => UpdState::UpToDate,
                Err(e) => UpdState::Error(e),
            };
            *st.lock().unwrap() = new;
        });
    }

    /// Скачивает установщик, проверяет подпись и запускает его; затем процесс завершается,
    /// а установщик заменяет файлы и снова запускает программу.
    pub fn install(&self) {
        let rel = {
            let mut s = self.0.lock().unwrap();
            let UpdState::Available(r) = s.clone() else { return };
            *s = UpdState::Downloading(r.clone());
            r
        };
        let st = self.0.clone();
        std::thread::spawn(move || {
            let result = switchdesk_update::download(&rel).and_then(|path| {
                let relaunch = std::env::current_exe().ok().map(|exe| (exe, Vec::new()));
                switchdesk_update::run_installer(&path, relaunch).map_err(|e| e.to_string())
            });
            match result {
                Ok(()) => {
                    crate::capture::set_sink(None);
                    std::process::exit(0);
                }
                Err(e) => *st.lock().unwrap() = UpdState::Error(e),
            }
        });
    }
}
