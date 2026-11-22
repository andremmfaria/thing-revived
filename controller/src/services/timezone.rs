//! Timezone commands: list, get and set through the `TimezoneCtl` port,
//! validating names against the cached zone list and persisting after a
//! successful set.

use crate::domain::timezone::validate_timezone;
use crate::ports::{EventSink, TimezoneCtl};
use crate::protocol::BridgeEvent;
use crate::services::settings::SettingsHolder;
use log::{error, info};
use std::sync::Arc;

pub struct TimezoneService {
    ctl: Arc<dyn TimezoneCtl>,
    zones: Vec<String>,
    settings: Arc<SettingsHolder>,
    sink: Arc<dyn EventSink>,
}

impl TimezoneService {
    /// Caches the valid zone list once: it changes only with a tzdata
    /// update, never at runtime.
    pub fn new(ctl: Arc<dyn TimezoneCtl>, settings: Arc<SettingsHolder>, sink: Arc<dyn EventSink>) -> TimezoneService {
        let zones = ctl.list();
        info!("timezone: {} valid IANA zones cached at startup", zones.len());
        TimezoneService { ctl, zones, settings, sink }
    }

    /// `timezone_get`.
    pub fn get(&self) {
        match self.ctl.get() {
            Ok(value) => self.sink.emit(BridgeEvent::Timezone { value }),
            Err(e) => self.sink.emit(BridgeEvent::SettingsError { context: "timezone_get".into(), message: e.message }),
        }
    }

    /// `timezone_list_get`.
    pub fn list(&self) {
        self.sink.emit(BridgeEvent::TimezoneList { zones: self.zones.clone() });
    }

    /// `timezone_set`: a name outside the cached list never reaches the
    /// command; persisted only after the system accepted it.
    pub fn set(&self, value: String) {
        match validate_timezone(&value, &self.zones).and_then(|()| self.ctl.set(&value)) {
            Ok(()) => {
                self.settings.update(|s| s.timezone = Some(value));
                self.get();
            }
            Err(e) => self.sink.emit(BridgeEvent::SettingsError { context: "timezone_set".into(), message: e.message }),
        }
    }

    /// `timedatectl set-timezone`'s own /etc/localtime symlink write does not
    /// survive a reboot on this read-only-rootfs image, so the saved zone is
    /// re-applied unconditionally at every startup.
    pub fn apply_saved(&self) {
        if let Some(tz) = self.settings.snapshot().timezone {
            match validate_timezone(&tz, &self.zones).and_then(|()| self.ctl.set(&tz)) {
                Ok(()) => info!("timezone: re-applied saved value {tz} at startup"),
                Err(e) => error!("timezone: failed to re-apply saved value {tz} at startup: {e}"),
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

    struct FakeTz {
        current: Mutex<String>,
        set_calls: Mutex<Vec<String>>,
    }

    impl TimezoneCtl for FakeTz {
        fn list(&self) -> Vec<String> {
            vec!["Europe/Lisbon".into(), "UTC".into()]
        }
        fn get(&self) -> Result<String, AppError> {
            Ok(self.current.lock().unwrap().clone())
        }
        fn set(&self, tz: &str) -> Result<(), AppError> {
            self.set_calls.lock().unwrap().push(tz.to_string());
            *self.current.lock().unwrap() = tz.to_string();
            Ok(())
        }
    }

    fn rig(initial: Settings) -> (TimezoneService, Arc<RecordingSink>, Arc<MemStore>, Arc<FakeTz>) {
        let sink = Arc::new(RecordingSink::new());
        let store = Arc::new(MemStore { initial, ..MemStore::default() });
        let settings = Arc::new(SettingsHolder::load(store.clone()));
        let ctl = Arc::new(FakeTz { current: Mutex::new("UTC".into()), set_calls: Mutex::new(vec![]) });
        (TimezoneService::new(ctl.clone(), settings, sink.clone()), sink, store, ctl)
    }

    #[test]
    fn list_and_get_report_to_clients() {
        let (svc, sink, _, _) = rig(Settings::default());
        svc.list();
        svc.get();
        let ev = sink.take();
        assert!(matches!(&ev[0], BridgeEvent::TimezoneList { zones } if zones.len() == 2));
        assert!(matches!(&ev[1], BridgeEvent::Timezone { value } if value == "UTC"));
    }

    #[test]
    fn set_persists_after_success() {
        let (svc, sink, store, ctl) = rig(Settings::default());
        svc.set("Europe/Lisbon".into());
        assert!(matches!(&sink.take()[..], [BridgeEvent::Timezone { value }] if value == "Europe/Lisbon"));
        assert_eq!(store.saved.lock().unwrap().clone(), vec![Settings { brightness: None, timezone: Some("Europe/Lisbon".into()) }]);
        assert_eq!(ctl.set_calls.lock().unwrap().clone(), vec!["Europe/Lisbon".to_string()]);
    }

    #[test]
    fn unknown_zone_never_reaches_the_system_or_the_file() {
        let (svc, sink, store, ctl) = rig(Settings::default());
        svc.set("UTC; rm -rf /".into());
        assert!(matches!(&sink.take()[..], [BridgeEvent::SettingsError { context, .. }] if context == "timezone_set"));
        assert!(ctl.set_calls.lock().unwrap().is_empty());
        assert!(store.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_reapplies_the_saved_zone() {
        let (svc, _, store, ctl) = rig(Settings { brightness: None, timezone: Some("Europe/Lisbon".into()) });
        svc.apply_saved();
        assert_eq!(ctl.set_calls.lock().unwrap().clone(), vec!["Europe/Lisbon".to_string()]);
        assert!(store.saved.lock().unwrap().is_empty());
    }
}
