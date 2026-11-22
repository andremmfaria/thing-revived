//! The in-memory copy of the persisted settings, shared by the display and
//! timezone services, and the rule for writing it back.

use crate::domain::settings::Settings;
use crate::ports::SettingsStore;
use crate::sync_util::lock;
use log::error;
use std::sync::{Arc, Mutex};

pub struct SettingsHolder {
    store: Arc<dyn SettingsStore>,
    current: Mutex<Settings>,
}

impl SettingsHolder {
    /// Loads the persisted settings (defaults if there are none).
    pub fn load(store: Arc<dyn SettingsStore>) -> SettingsHolder {
        let current = store.load();
        SettingsHolder { store, current: Mutex::new(current) }
    }

    pub fn snapshot(&self) -> Settings {
        lock(&self.current).clone()
    }

    /// Applies `change`, then persists. Called only after the device call
    /// succeeded, so a failed set never reaches the file. A failed write is
    /// logged, not surfaced: the setting itself did take effect.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) {
        let mut guard = lock(&self.current);
        change(&mut guard);
        if let Err(e) = self.store.save(&guard) {
            error!("settings: failed to persist: {e}");
        }
    }
}

pub mod testing {
    use super::*;
    use crate::app::error::AppError;

    /// An in-memory store counting saves.
    #[derive(Default)]
    pub struct MemStore {
        pub initial: Settings,
        pub saved: Mutex<Vec<Settings>>,
        pub fail_save: bool,
    }

    impl SettingsStore for MemStore {
        fn load(&self) -> Settings {
            self.initial.clone()
        }

        fn save(&self, s: &Settings) -> Result<(), AppError> {
            if self.fail_save {
                return Err(AppError::failed("disk full"));
            }
            lock(&self.saved).push(s.clone());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MemStore;
    use super::*;

    #[test]
    fn update_persists_the_whole_settings_value() {
        let store = Arc::new(MemStore::default());
        let holder = SettingsHolder::load(store.clone());
        holder.update(|s| s.brightness = Some(5));
        holder.update(|s| s.timezone = Some("UTC".into()));
        let saved = lock(&store.saved).clone();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1], Settings { brightness: Some(5), timezone: Some("UTC".into()) });
        assert_eq!(holder.snapshot(), saved[1]);
    }

    #[test]
    fn a_failed_write_keeps_the_in_memory_value() {
        let store = Arc::new(MemStore { fail_save: true, ..MemStore::default() });
        let holder = SettingsHolder::load(store);
        holder.update(|s| s.brightness = Some(9));
        assert_eq!(holder.snapshot().brightness, Some(9));
    }
}
