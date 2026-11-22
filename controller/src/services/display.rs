//! Brightness commands: read and set through the `Backlight` port, persist
//! after a successful set, and report the outcome to clients.

use crate::ports::{Backlight, EventSink};
use crate::protocol::BridgeEvent;
use crate::services::settings::SettingsHolder;
use log::{error, info};
use std::sync::Arc;

pub struct DisplayService {
    backlight: Arc<dyn Backlight>,
    settings: Arc<SettingsHolder>,
    sink: Arc<dyn EventSink>,
}

impl DisplayService {
    pub fn new(backlight: Arc<dyn Backlight>, settings: Arc<SettingsHolder>, sink: Arc<dyn EventSink>) -> DisplayService {
        DisplayService { backlight, settings, sink }
    }

    /// `brightness_get`.
    pub fn get(&self) {
        match (self.backlight.get(), self.backlight.max()) {
            (Ok(value), Ok(max)) => self.sink.emit(BridgeEvent::Brightness { value, max }),
            (res1, res2) => {
                let message = res1.and(res2).unwrap_err().message;
                self.sink.emit(BridgeEvent::SettingsError { context: "brightness_get".into(), message });
            }
        }
    }

    /// `brightness_set`: persisted only after the device accepted the value.
    pub fn set(&self, value: u32) {
        match self.backlight.set(value) {
            Ok(()) => {
                self.settings.update(|s| s.brightness = Some(value));
                self.get();
            }
            Err(e) => self.sink.emit(BridgeEvent::SettingsError { context: "brightness_set".into(), message: e.message }),
        }
    }

    /// Brightness persistence (design doc section 8.1): systemd-backlight is
    /// present on this image but its saved-state directory was found empty on
    /// a live check, so its "save" half is unconfirmed. Re-applying the
    /// agent's own persisted value at startup is belt and suspenders, not a
    /// replacement.
    pub fn apply_saved(&self) {
        if let Some(value) = self.settings.snapshot().brightness {
            match self.backlight.set(value) {
                Ok(()) => info!("brightness: re-applied saved value {value} at startup"),
                Err(e) => error!("brightness: failed to re-apply saved value {value} at startup: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::error::AppError;
    use crate::app::hub::RecordingSink;
    use crate::domain::settings::Settings;
    use crate::services::settings::testing::MemStore;
    use std::sync::Mutex;

    struct FakeBacklight {
        max: u32,
        current: Mutex<u32>,
    }

    impl Backlight for FakeBacklight {
        fn max(&self) -> Result<u32, AppError> {
            Ok(self.max)
        }
        fn get(&self) -> Result<u32, AppError> {
            Ok(*self.current.lock().unwrap())
        }
        fn set(&self, v: u32) -> Result<(), AppError> {
            crate::domain::brightness::validate(v, self.max)?;
            *self.current.lock().unwrap() = v;
            Ok(())
        }
    }

    fn rig(initial: Settings) -> (DisplayService, Arc<RecordingSink>, Arc<MemStore>) {
        let sink = Arc::new(RecordingSink::new());
        let store = Arc::new(MemStore { initial, ..MemStore::default() });
        let settings = Arc::new(SettingsHolder::load(store.clone()));
        let backlight = Arc::new(FakeBacklight { max: 160, current: Mutex::new(10) });
        (DisplayService::new(backlight, settings, sink.clone()), sink, store)
    }

    #[test]
    fn get_reports_value_and_max() {
        let (svc, sink, _) = rig(Settings::default());
        svc.get();
        assert!(matches!(sink.take()[..], [BridgeEvent::Brightness { value: 10, max: 160 }]));
    }

    #[test]
    fn set_persists_after_success_then_reports() {
        let (svc, sink, store) = rig(Settings::default());
        svc.set(80);
        assert!(matches!(sink.take()[..], [BridgeEvent::Brightness { value: 80, max: 160 }]));
        let saved = store.saved.lock().unwrap().clone();
        assert_eq!(saved, vec![Settings { brightness: Some(80), timezone: None }]);
    }

    #[test]
    fn rejected_set_is_not_persisted_and_reports_a_settings_error() {
        let (svc, sink, store) = rig(Settings::default());
        svc.set(200);
        let events = sink.take();
        assert!(matches!(&events[..], [BridgeEvent::SettingsError { context, message }]
            if context == "brightness_set" && message == "brightness 200 out of range 0..=160"));
        assert!(store.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_reapplies_the_saved_value_without_resaving() {
        let (svc, sink, store) = rig(Settings { brightness: Some(33), timezone: None });
        svc.apply_saved();
        svc.get();
        assert!(matches!(sink.take()[..], [BridgeEvent::Brightness { value: 33, .. }]));
        assert!(store.saved.lock().unwrap().is_empty());
    }
}
