//! `DeviceSettingsService`: `device_settings_get`, `device_settings_set` and
//! `device_reboot` (plan sections 3, 5 and 6). It owns the whole state machine
//! and the exclusion invariant (`mode == host` implies `adb == off`) under one
//! mutation mutex; the persisted values live in the settings file (through the
//! `DeviceSettingsStore` port) and the live values are always read fresh from
//! the system (through `DeviceCtl`), never remembered.
//!
//! Order of checks on `device_settings_set`, exactly, before anything changes:
//! admin session unlocked, then the `setting` and `value` whitelist, then the
//! host-forces-adb-off invariant, then the per-connection rate limit, then the
//! mutation mutex (a second request is `busy`, never queued). Past all of that
//! the safe order of plan 5.2 applies: restrict first, loosen last. Disables
//! persist before applying live; enables apply live first, persist second and
//! roll the live change back when the persist fails. The truth is always
//! re-queried and broadcast afterwards, success or failure, so the badge never
//! shows a guess. There is no `confirm` echo: the hold gesture is a frontend
//! concern only.

use crate::app::hub::ConnId;
use crate::domain::device_settings::{self as ds, apply_error_code, DeviceSettings, ErrorCode, Loaded, Setting, UsbRole};
use crate::ports::{AdminSessionGate, DeviceCtl, DeviceSettingsStore, EventSink};
use crate::protocol::{BridgeEvent, DeviceSettingsValuesWire};
use crate::sync_util::lock;
use crate::utils::types::AppError;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The values the file holds plus what is needed to write them back.
struct Persisted {
    settings: DeviceSettings,
    image_id: String,
    /// False when the store is unusable: every set answers `persist_failed`.
    persistable: bool,
}

pub struct DeviceSettingsService {
    store: Arc<dyn DeviceSettingsStore>,
    ctl: Arc<dyn DeviceCtl>,
    gate: Arc<dyn AdminSessionGate>,
    sink: Arc<dyn EventSink>,
    /// One mutation (or reboot) at a time; held for the whole apply.
    mutate: Mutex<()>,
    persisted: Mutex<Persisted>,
    /// The setting being applied right now, for the `busy` field.
    busy: Mutex<Option<Setting>>,
    /// Per-connection mutation timestamps for the rate limiter.
    rate: Mutex<HashMap<u64, Vec<Instant>>>,
    rate_max: usize,
    rate_window: Duration,
}

impl DeviceSettingsService {
    /// `loaded` is the result of the startup load (and repair); `rate_max` is
    /// `[device_settings] mutations_per_minute` (6).
    pub fn new(
        store: Arc<dyn DeviceSettingsStore>,
        ctl: Arc<dyn DeviceCtl>,
        gate: Arc<dyn AdminSessionGate>,
        sink: Arc<dyn EventSink>,
        loaded: Loaded,
        rate_max: usize,
    ) -> DeviceSettingsService {
        DeviceSettingsService::with_rate_window(store, ctl, gate, sink, loaded, rate_max, Duration::from_secs(60))
    }

    /// Separate so tests can use a window short enough to wait out.
    pub fn with_rate_window(
        store: Arc<dyn DeviceSettingsStore>,
        ctl: Arc<dyn DeviceCtl>,
        gate: Arc<dyn AdminSessionGate>,
        sink: Arc<dyn EventSink>,
        loaded: Loaded,
        rate_max: usize,
        rate_window: Duration,
    ) -> DeviceSettingsService {
        DeviceSettingsService {
            store,
            ctl,
            gate,
            sink,
            mutate: Mutex::new(()),
            persisted: Mutex::new(Persisted { settings: loaded.settings, image_id: loaded.image_id, persistable: loaded.persistable }),
            busy: Mutex::new(None),
            rate: Mutex::new(HashMap::new()),
            rate_max,
            rate_window,
        }
    }

    // -----------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------

    fn persisted_settings(&self) -> DeviceSettings {
        lock(&self.persisted).settings
    }

    fn live_flag(&self, setting: Setting) -> bool {
        self.ctl.is_on(setting).unwrap_or_else(|e| {
            warn!("device settings: status({}) failed, reporting off: {e}", setting.as_str());
            false
        })
    }

    /// The running USB role; the persisted one when the tool cannot say.
    fn live_mode(&self) -> UsbRole {
        match self.ctl.usb_status() {
            Ok(s) => s.role.unwrap_or_else(|| self.persisted_settings().mode()),
            Err(e) => {
                warn!("device settings: usb status failed, reporting the persisted mode: {e}");
                self.persisted_settings().mode()
            }
        }
    }

    /// Re-queries the live values fresh and broadcasts them with the persisted
    /// ones (plan section 3.2); never cached.
    fn report(&self) {
        let mode = self.live_mode();
        let ssh = self.live_flag(Setting::Ssh);
        let adb = self.live_flag(Setting::Adb);
        let uart = self.live_flag(Setting::Uart);
        let p = self.persisted_settings();
        let busy = *lock(&self.busy);
        // `adb_blocked` follows the persisted mode (the invariant's source of
        // truth) and also the live one, so a drifted live host still greys adb.
        let adb_blocked = if mode == UsbRole::Host { Some("mode_host".to_string()) } else { p.adb_blocked().map(String::from) };
        self.sink.emit(BridgeEvent::DeviceSettings {
            mode: mode.as_str().into(),
            ssh: ds::on_off(ssh).into(),
            adb: ds::on_off(adb).into(),
            uart: ds::on_off(uart).into(),
            persisted: DeviceSettingsValuesWire {
                mode: p.value_str(Setting::Mode).into(),
                ssh: p.value_str(Setting::Ssh).into(),
                adb: p.value_str(Setting::Adb).into(),
                uart: p.value_str(Setting::Uart).into(),
            },
            adb_blocked,
            busy: busy.map(|s| s.as_str().to_string()),
        });
    }

    /// `device_settings_get`. Ungated: the badge needs live adb and uart while
    /// locked.
    pub fn get(&self) {
        self.report();
    }

    fn fail(&self, context: &str, code: ErrorCode, message: impl Into<String>) {
        self.sink.emit(BridgeEvent::DeviceSettingsError { context: context.into(), code: code.as_str().into(), message: message.into() });
    }

    fn rate_limited(&self, conn: ConnId) -> bool {
        let now = Instant::now();
        let mut guard = lock(&self.rate);
        let hits = guard.entry(conn.0).or_default();
        hits.retain(|t| now.duration_since(*t) < self.rate_window);
        if hits.len() >= self.rate_max {
            return true;
        }
        hits.push(now);
        false
    }

    // -----------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------

    /// `device_settings_set`. `setting` and `value` are untrusted client
    /// strings.
    pub fn set(&self, conn: ConnId, setting: &str, value: &str) {
        const CTX: &str = "device_settings_set";

        // (1) Admin session, before anything else, including parsing.
        if self.gate.require(conn).is_err() {
            return self.fail(CTX, ErrorCode::NotUnlocked, "Unlock the device settings first");
        }
        // (2) Whitelists, then the value applied to a scratch copy through the
        // same domain path the invariant lives in.
        let setting = match Setting::validate(setting) {
            Ok(s) => s,
            Err(code) => return self.fail(CTX, code, "Unknown setting"),
        };
        let mut wanted = self.persisted_settings();
        match wanted.apply_value(setting, value) {
            Ok(()) => {}
            // (3) Host forces adb off: adb on is refused before it counts
            // against the rate limit. `apply_value` checks the persisted mode;
            // the live mode is checked next.
            Err(code @ ErrorCode::AdbBlockedHost) => return self.fail(CTX, code, "Switch the USB port to device mode first"),
            Err(code) => return self.fail(CTX, code, "Invalid value"),
        }
        if setting == Setting::Adb && wanted.adb() && self.live_mode() == UsbRole::Host {
            return self.fail(CTX, ErrorCode::AdbBlockedHost, "Switch the USB port to device mode first");
        }
        // (4) Rate limit.
        if self.rate_limited(conn) {
            return self.fail(CTX, ErrorCode::RateLimited, "Too many changes. Wait and try again");
        }
        // (5) One mutation at a time.
        let Ok(_guard) = self.mutate.try_lock() else {
            return self.fail(CTX, ErrorCode::Busy, "Another device settings change is already in progress");
        };
        *lock(&self.busy) = Some(setting);
        let _clear = BusyGuard(&self.busy);
        self.report();

        // (6) Apply in the safe order, then (7) always re-read and broadcast.
        if let Err((code, message)) = self.apply(setting, value) {
            warn!("{CTX}: {} refused or failed ({}): {message}", setting.as_str(), code.as_str());
            self.fail(CTX, code, message);
        } else {
            info!("device settings: {} set to {} (conn {})", setting.as_str(), value, conn.0);
        }
        *lock(&self.busy) = None;
        self.report();
    }

    /// Runs the plan 5.2 sequence for one setting. Caller holds `mutate`.
    fn apply(&self, setting: Setting, value: &str) -> Result<(), (ErrorCode, String)> {
        let current = self.persisted_settings();
        if !lock(&self.persisted).persistable {
            return Err((ErrorCode::PersistFailed, "The settings file cannot be written".into()));
        }
        let mut wanted = current;
        // Re-checked under the lock against the persisted mode: it may have
        // changed while this request waited for the earlier checks.
        wanted.apply_value(setting, value).map_err(|c| (c, "Switch the USB port to device mode first".to_string()))?;
        match setting {
            Setting::Mode if wanted.mode() == UsbRole::Host => self.apply_host(current, wanted),
            Setting::Mode => self.apply_device(wanted),
            _ => {
                let on = wanted.flag(setting).unwrap_or(false);
                if on {
                    self.apply_enable(setting, current, wanted)
                } else {
                    self.apply_disable(setting, wanted)
                }
            }
        }
    }

    fn apply_failed(e: &AppError) -> (ErrorCode, String) {
        (apply_error_code(e), e.message.clone())
    }

    /// Writes `settings` and updates the in-memory copy only on success.
    fn persist(&self, settings: DeviceSettings) -> Result<(), (ErrorCode, String)> {
        let image_id = lock(&self.persisted).image_id.clone();
        match self.store.save(&image_id, &settings) {
            Ok(()) => {
                lock(&self.persisted).settings = settings;
                Ok(())
            }
            Err(e) => Err((ErrorCode::PersistFailed, e.message)),
        }
    }

    fn mirror_boot_role(&self, role: UsbRole) {
        if let Err(e) = self.ctl.persist_boot_role(role) {
            warn!("device settings: boot-role mirror ({}) failed: {e}", role.as_str());
        }
    }

    /// Host: adb off live, one atomic persist of `{host, adb off}`, mirror the
    /// boot role, then the role switch.
    fn apply_host(&self, current: DeviceSettings, wanted: DeviceSettings) -> Result<(), (ErrorCode, String)> {
        if self.live_flag(Setting::Adb) {
            self.ctl.set_live(Setting::Adb, false).map_err(|e| Self::apply_failed(&e))?;
        }
        self.persist(wanted)?;
        self.mirror_boot_role(UsbRole::Host);
        if let Err(e) = self.ctl.to_host() {
            // Back to device; adb stays off (the safe direction).
            let mut back = wanted;
            back.set_mode(UsbRole::Device);
            if let Err((_, m)) = self.persist(back) {
                warn!("device settings: could not persist the mode rollback: {m}");
            }
            self.mirror_boot_role(current.mode());
            return Err(Self::apply_failed(&e));
        }
        Ok(())
    }

    /// Device: persist, mirror, switch, rebind. adb stays off.
    fn apply_device(&self, wanted: DeviceSettings) -> Result<(), (ErrorCode, String)> {
        self.persist(wanted)?;
        self.mirror_boot_role(UsbRole::Device);
        self.ctl.to_device().map_err(|e| Self::apply_failed(&e))
    }

    /// Enables (adb, ssh, uart): live first, persist second, roll back live if
    /// the persist fails.
    fn apply_enable(&self, setting: Setting, _current: DeviceSettings, wanted: DeviceSettings) -> Result<(), (ErrorCode, String)> {
        if let Err(e) = self.ctl.set_live(setting, true) {
            // Never leave a half-applied enable reachable.
            if let Err(e2) = self.ctl.set_live(setting, false) {
                warn!("device settings: best-effort disable after a failed {} enable also failed: {e2}", setting.as_str());
            }
            return Err(Self::apply_failed(&e));
        }
        if let Err(err) = self.persist(wanted) {
            if let Err(e2) = self.ctl.set_live(setting, false) {
                warn!("device settings: rollback of {} after a failed persist also failed: {e2}", setting.as_str());
            }
            return Err(err);
        }
        Ok(())
    }

    /// Disables (adb, ssh, uart): persist first, then apply live. A failed
    /// persist still applies the disable (the safe direction) and reports
    /// `persist_failed`; the reply carries the true live state.
    fn apply_disable(&self, setting: Setting, wanted: DeviceSettings) -> Result<(), (ErrorCode, String)> {
        let persisted = self.persist(wanted);
        let live = self.ctl.set_live(setting, false).map_err(|e| Self::apply_failed(&e));
        persisted.and(live)
    }

    // -----------------------------------------------------------------
    // Reboot
    // -----------------------------------------------------------------

    /// `device_reboot`: gated by the admin session and refused while a
    /// mutation is in flight (it would race the unit starts and stops).
    pub fn reboot(&self, conn: ConnId) {
        const CTX: &str = "device_reboot";
        if self.gate.require(conn).is_err() {
            return self.fail(CTX, ErrorCode::NotUnlocked, "Unlock the device settings first");
        }
        let Ok(_guard) = self.mutate.try_lock() else {
            return self.fail(CTX, ErrorCode::Busy, "A device settings change is in progress; try the reboot again shortly");
        };
        match self.ctl.reboot() {
            Ok(()) => {
                info!("device settings: reboot requested (conn {})", conn.0);
                self.sink.emit(BridgeEvent::DeviceRebooting {});
            }
            Err(e) => {
                warn!("{CTX}: {e}");
                self.fail(CTX, apply_error_code(&e), e.message.clone());
            }
        }
    }
}

/// Clears the busy marker when a mutation ends by any path.
struct BusyGuard<'a>(&'a Mutex<Option<Setting>>);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        *lock(self.0) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hub::RecordingSink;
    use crate::domain::device_settings::UsbRoleStatus;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex as StdMutex;

    /// Fake system: records every call in order, keeps live state, and can
    /// fail or block a named call.
    struct FakeCtl {
        calls: Arc<StdMutex<Vec<String>>>,
        live: StdMutex<HashMap<&'static str, bool>>,
        role: StdMutex<UsbRole>,
        fail: StdMutex<Vec<String>>,
        /// When set, `set_live` signals `entered` then waits on `release`.
        entered: StdMutex<Option<Sender<()>>>,
        release: StdMutex<Option<Receiver<()>>>,
    }

    impl FakeCtl {
        fn new(calls: Arc<StdMutex<Vec<String>>>) -> Arc<FakeCtl> {
            Arc::new(FakeCtl {
                calls,
                live: StdMutex::new([("ssh", true), ("adb", false), ("uart", false)].into_iter().collect()),
                role: StdMutex::new(UsbRole::Device),
                fail: StdMutex::new(vec![]),
                entered: StdMutex::new(None),
                release: StdMutex::new(None),
            })
        }
        fn record(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }
        fn failing(&self, call: &str) -> bool {
            self.fail.lock().unwrap().iter().any(|c| c == call)
        }
        fn fail_on(&self, call: &str) {
            self.fail.lock().unwrap().push(call.to_string());
        }
    }

    impl DeviceCtl for FakeCtl {
        fn usb_status(&self) -> Result<UsbRoleStatus, AppError> {
            Ok(UsbRoleStatus { role: Some(*self.role.lock().unwrap()), boot_role: None })
        }
        fn is_on(&self, setting: Setting) -> Result<bool, AppError> {
            Ok(*self.live.lock().unwrap().get(setting.as_str()).unwrap_or(&false))
        }
        fn set_live(&self, setting: Setting, on: bool) -> Result<(), AppError> {
            let name = format!("{} {}", setting.as_str(), ds::on_off(on));
            self.record(format!("live {name}"));
            let release = self.release.lock().unwrap().take();
            if let Some(rx) = release {
                if let Some(tx) = &*self.entered.lock().unwrap() {
                    let _ = tx.send(());
                }
                let _ = rx.recv();
            }
            if self.failing(&name) {
                return Err(AppError::failed("tool failed"));
            }
            self.live.lock().unwrap().insert(setting.as_str(), on);
            Ok(())
        }
        fn to_host(&self) -> Result<(), AppError> {
            self.record("to_host".into());
            if self.failing("to_host") {
                return Err(AppError::failed("role switch failed"));
            }
            *self.role.lock().unwrap() = UsbRole::Host;
            Ok(())
        }
        fn to_device(&self) -> Result<(), AppError> {
            self.record("to_device".into());
            *self.role.lock().unwrap() = UsbRole::Device;
            Ok(())
        }
        fn persist_boot_role(&self, role: UsbRole) -> Result<(), AppError> {
            self.record(format!("boot-role {}", role.as_str()));
            Ok(())
        }
        fn reboot(&self) -> Result<(), AppError> {
            self.record("reboot".into());
            Ok(())
        }
    }

    struct FakeStore {
        calls: Arc<StdMutex<Vec<String>>>,
        saved: StdMutex<Vec<DeviceSettings>>,
        fail: StdMutex<bool>,
    }

    impl DeviceSettingsStore for FakeStore {
        fn load(&self) -> Loaded {
            unreachable!("the service never reloads")
        }
        fn save(&self, _image_id: &str, s: &DeviceSettings) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(format!("save {} {} {} {}", s.mode().as_str(), s.value_str(Setting::Ssh), s.value_str(Setting::Adb), s.value_str(Setting::Uart)));
            if *self.fail.lock().unwrap() {
                return Err(AppError::failed("disk full"));
            }
            self.saved.lock().unwrap().push(*s);
            Ok(())
        }
    }

    struct FakeGate {
        locked: StdMutex<bool>,
    }

    impl AdminSessionGate for FakeGate {
        fn require(&self, _conn: ConnId) -> Result<String, AppError> {
            if *self.locked.lock().unwrap() {
                Err(AppError::invalid("not_unlocked"))
            } else {
                Ok("kiosk".into())
            }
        }
    }

    const CONN: ConnId = ConnId(1);
    const OTHER: ConnId = ConnId(2);

    struct Rig {
        svc: Arc<DeviceSettingsService>,
        sink: Arc<RecordingSink>,
        ctl: Arc<FakeCtl>,
        store: Arc<FakeStore>,
        gate: Arc<FakeGate>,
        calls: Arc<StdMutex<Vec<String>>>,
    }

    impl Rig {
        fn new() -> Rig {
            Rig::with(DeviceSettings::new(UsbRole::Device, true, false, false), 6, Duration::from_secs(60))
        }

        fn with(start: DeviceSettings, rate_max: usize, window: Duration) -> Rig {
            let calls = Arc::new(StdMutex::new(vec![]));
            let sink = Arc::new(RecordingSink::new());
            let ctl = FakeCtl::new(calls.clone());
            let store = Arc::new(FakeStore { calls: calls.clone(), saved: StdMutex::new(vec![]), fail: StdMutex::new(false) });
            let gate = Arc::new(FakeGate { locked: StdMutex::new(false) });
            *ctl.role.lock().unwrap() = start.mode();
            {
                let mut live = ctl.live.lock().unwrap();
                live.insert("ssh", start.ssh());
                live.insert("adb", start.adb());
                live.insert("uart", start.uart());
            }
            let loaded = Loaded { settings: start, image_id: "img".into(), write: false, quarantine: false, persistable: true, notes: vec![] };
            let svc = Arc::new(DeviceSettingsService::with_rate_window(
                store.clone(),
                ctl.clone(),
                gate.clone(),
                sink.clone(),
                loaded,
                rate_max,
                window,
            ));
            Rig { svc, sink, ctl, store, gate, calls }
        }

        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }

        fn events(&self) -> Vec<BridgeEvent> {
            self.sink.take()
        }
    }

    fn error_code(events: &[BridgeEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            BridgeEvent::DeviceSettingsError { code, .. } => Some(code.clone()),
            _ => None,
        })
    }

    struct State {
        mode: String,
        ssh: String,
        adb: String,
        uart: String,
        p: DeviceSettingsValuesWire,
        blocked: Option<String>,
        busy: Option<String>,
    }

    fn last_state(events: &[BridgeEvent]) -> State {
        events
            .iter()
            .rev()
            .find_map(|e| match e {
                BridgeEvent::DeviceSettings { mode, ssh, adb, uart, persisted, adb_blocked, busy } => Some(State {
                    mode: mode.clone(),
                    ssh: ssh.clone(),
                    adb: adb.clone(),
                    uart: uart.clone(),
                    p: persisted.clone(),
                    blocked: adb_blocked.clone(),
                    busy: busy.clone(),
                }),
                _ => None,
            })
            .expect("a device_settings event")
    }

    #[test]
    fn get_is_ungated_and_reports_live_and_persisted() {
        let rig = Rig::new();
        *rig.gate.locked.lock().unwrap() = true;
        rig.svc.get();
        let st = last_state(&rig.events());
        assert_eq!((st.mode.as_str(), st.ssh.as_str(), st.adb.as_str(), st.uart.as_str()), ("device", "on", "off", "off"));
        assert_eq!((st.p.mode.as_str(), st.p.ssh.as_str(), st.p.adb.as_str(), st.p.uart.as_str()), ("device", "on", "off", "off"));
        assert_eq!((st.blocked, st.busy), (None, None));
        assert!(rig.calls().is_empty(), "a read never mutates");
    }

    #[test]
    fn a_locked_session_is_refused_before_anything_else() {
        let rig = Rig::new();
        *rig.gate.locked.lock().unwrap() = true;
        rig.svc.set(CONN, "bogus", "bogus");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("not_unlocked"), "the gate comes before the whitelist");
        rig.svc.reboot(CONN);
        assert_eq!(error_code(&rig.events()).as_deref(), Some("not_unlocked"));
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn the_session_expiring_between_two_commands_refuses_the_second() {
        let rig = Rig::new();
        rig.svc.set(CONN, "uart", "on");
        assert_eq!(error_code(&rig.events()), None);
        *rig.gate.locked.lock().unwrap() = true;
        rig.calls();
        rig.svc.set(CONN, "uart", "off");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("not_unlocked"));
        assert!(rig.calls().is_empty(), "nothing ran after expiry");
    }

    #[test]
    fn unknown_setting_and_value_are_refused_without_touching_anything() {
        let rig = Rig::new();
        for (s, v, code) in [
            ("bogus", "on", "invalid_setting"),
            ("SSH", "on", "invalid_setting"),
            ("", "on", "invalid_setting"),
            ("ssh", "ON", "invalid_value"),
            ("ssh", "true", "invalid_value"),
            ("mode", "on", "invalid_value"),
            ("uart", "host", "invalid_value"),
        ] {
            rig.svc.set(CONN, s, v);
            assert_eq!(error_code(&rig.events()).as_deref(), Some(code), "{s}={v}");
        }
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn adb_on_in_host_mode_is_blocked_before_any_call_and_before_the_rate_limit() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Host, true, false, false), 1, Duration::from_secs(60));
        for _ in 0..3 {
            rig.svc.set(CONN, "adb", "on");
            assert_eq!(error_code(&rig.events()).as_deref(), Some("adb_blocked_host"), "never counted against the limiter");
        }
        assert!(rig.calls().is_empty());
        rig.svc.get();
        let state = last_state(&rig.events());
        assert_eq!(state.blocked.as_deref(), Some("mode_host"));
    }

    #[test]
    fn adb_on_is_also_blocked_when_only_the_live_mode_is_host() {
        let rig = Rig::new();
        *rig.ctl.role.lock().unwrap() = UsbRole::Host;
        rig.svc.set(CONN, "adb", "on");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("adb_blocked_host"));
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn mode_host_turns_adb_off_live_then_persists_one_write_then_switches() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, true, false), 6, Duration::from_secs(60));
        rig.svc.set(CONN, "mode", "host");
        assert_eq!(rig.calls(), vec!["live adb off", "save host on off off", "boot-role host", "to_host"]);
        let events = rig.events();
        assert_eq!(error_code(&events), None);
        let st = last_state(&events);
        assert_eq!((st.mode.as_str(), st.adb.as_str(), st.p.mode.as_str(), st.p.adb.as_str()), ("host", "off", "host", "off"));
        assert_eq!((st.blocked.as_deref(), st.busy), (Some("mode_host"), None));
    }

    #[test]
    fn mode_host_failing_adb_off_changes_nothing_persisted() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, true, false), 6, Duration::from_secs(60));
        rig.ctl.fail_on("adb off");
        rig.svc.set(CONN, "mode", "host");
        assert_eq!(rig.calls(), vec!["live adb off"], "no save, no role switch");
        let events = rig.events();
        assert_eq!(error_code(&events).as_deref(), Some("apply_failed"));
        assert_eq!(last_state(&events).p.mode, "device");
        assert!(rig.store.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn mode_host_role_switch_failure_persists_device_back_with_adb_still_off() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, true, false), 6, Duration::from_secs(60));
        rig.ctl.fail_on("to_host");
        rig.svc.set(CONN, "mode", "host");
        assert_eq!(
            rig.calls(),
            vec!["live adb off", "save host on off off", "boot-role host", "to_host", "save device on off off", "boot-role device"]
        );
        let events = rig.events();
        assert_eq!(error_code(&events).as_deref(), Some("apply_failed"));
        let p = last_state(&events).p;
        assert_eq!((p.mode.as_str(), p.adb.as_str()), ("device", "off"));
    }

    #[test]
    fn mode_host_persist_failure_leaves_adb_off_live_and_reports_the_truth() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, true, false), 6, Duration::from_secs(60));
        *rig.store.fail.lock().unwrap() = true;
        rig.svc.set(CONN, "mode", "host");
        assert_eq!(rig.calls(), vec!["live adb off", "save host on off off"], "no role switch after a failed persist");
        let events = rig.events();
        assert_eq!(error_code(&events).as_deref(), Some("persist_failed"));
        let st = last_state(&events);
        assert_eq!((st.mode.as_str(), st.adb.as_str()), ("device", "off"), "live adb is already off, the safe direction");
    }

    #[test]
    fn mode_device_persists_first_then_switches_and_adb_stays_off() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Host, true, false, false), 6, Duration::from_secs(60));
        rig.svc.set(CONN, "mode", "device");
        assert_eq!(rig.calls(), vec!["save device on off off", "boot-role device", "to_device"]);
        let st = last_state(&rig.events());
        assert_eq!((st.mode.as_str(), st.adb.as_str(), st.p.adb.as_str(), st.blocked), ("device", "off", "off", None));
    }

    #[test]
    fn enables_apply_live_first_and_persist_second() {
        let rig = Rig::new();
        for s in ["adb", "ssh", "uart"] {
            rig.svc.set(CONN, s, "on");
        }
        let calls = rig.calls();
        assert_eq!(calls[0], "live adb on");
        assert!(calls[1].starts_with("save device on on off"));
        assert_eq!(calls[2], "live ssh on");
        assert_eq!(calls[4], "live uart on");
        assert_eq!(calls[5], "save device on on on");
        assert_eq!(error_code(&rig.events()), None);
    }

    #[test]
    fn disables_persist_first_and_apply_live_second() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, true, true), 6, Duration::from_secs(60));
        for s in ["adb", "ssh", "uart"] {
            rig.svc.set(CONN, s, "off");
        }
        assert_eq!(
            rig.calls(),
            vec![
                "save device on off on",
                "live adb off",
                "save device off off on",
                "live ssh off",
                "save device off off off",
                "live uart off"
            ]
        );
    }

    #[test]
    fn a_failed_persist_after_an_enable_rolls_the_live_change_back() {
        for s in ["adb", "ssh", "uart"] {
            let rig = Rig::new();
            *rig.store.fail.lock().unwrap() = true;
            rig.svc.set(CONN, s, "on");
            let calls = rig.calls();
            assert_eq!(calls.first().map(String::as_str), Some(format!("live {s} on").as_str()));
            assert_eq!(calls.last().map(String::as_str), Some(format!("live {s} off").as_str()), "rolled back: {s}");
            let events = rig.events();
            assert_eq!(error_code(&events).as_deref(), Some("persist_failed"));
            let state = last_state(&events);
            let live = match s {
                "adb" => &state.adb,
                "ssh" => &state.ssh,
                _ => &state.uart,
            };
            assert_eq!(live, "off", "live shows the rollback");
        }
    }

    #[test]
    fn a_failed_ssh_enable_rolls_back_and_reports_apply_failed() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, false, false, false), 6, Duration::from_secs(60));
        rig.ctl.fail_on("ssh on");
        rig.svc.set(CONN, "ssh", "on");
        assert_eq!(rig.calls(), vec!["live ssh on", "live ssh off"], "nothing persisted");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("apply_failed"));
    }

    #[test]
    fn a_failed_disable_keeps_the_persisted_off_and_reports_live_truth() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, false, true), 6, Duration::from_secs(60));
        rig.ctl.fail_on("uart off");
        rig.svc.set(CONN, "uart", "off");
        let events = rig.events();
        assert_eq!(error_code(&events).as_deref(), Some("apply_failed"));
        let state = last_state(&events);
        assert_eq!((state.uart.as_str(), state.p.uart.as_str()), ("on", "off"), "drift: live on, persisted off, never under-reported");
    }

    #[test]
    fn a_failed_persist_on_a_disable_still_applies_the_disable() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, false, true), 6, Duration::from_secs(60));
        *rig.store.fail.lock().unwrap() = true;
        rig.svc.set(CONN, "uart", "off");
        assert_eq!(rig.calls(), vec!["save device on off off", "live uart off"]);
        let events = rig.events();
        assert_eq!(error_code(&events).as_deref(), Some("persist_failed"));
        assert_eq!(last_state(&events).uart, "off");
    }

    #[test]
    fn the_seventh_change_in_a_window_is_rate_limited_per_connection() {
        let rig = Rig::new();
        for i in 0..6 {
            rig.svc.set(CONN, "ssh", if i % 2 == 0 { "off" } else { "on" });
            assert_eq!(error_code(&rig.events()), None);
        }
        rig.calls();
        rig.svc.set(CONN, "ssh", "off");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("rate_limited"));
        assert!(rig.calls().is_empty());
        rig.svc.set(OTHER, "ssh", "off");
        assert_eq!(error_code(&rig.events()), None, "another connection has its own budget");
    }

    #[test]
    fn the_rate_limit_window_expires() {
        let rig = Rig::with(DeviceSettings::new(UsbRole::Device, true, false, false), 1, Duration::from_millis(60));
        rig.svc.set(CONN, "ssh", "off");
        rig.events();
        rig.svc.set(CONN, "ssh", "on");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("rate_limited"));
        std::thread::sleep(Duration::from_millis(90));
        rig.svc.set(CONN, "ssh", "on");
        assert_eq!(error_code(&rig.events()), None);
    }

    #[test]
    fn a_second_change_while_one_runs_is_busy_and_a_reboot_is_refused() {
        let rig = Rig::new();
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        *rig.ctl.entered.lock().unwrap() = Some(entered_tx);
        *rig.ctl.release.lock().unwrap() = Some(release_rx);
        let svc = rig.svc.clone();
        let worker = std::thread::spawn(move || svc.set(CONN, "uart", "on"));
        entered_rx.recv().unwrap();
        rig.events();

        rig.svc.set(OTHER, "ssh", "off");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("busy"));
        rig.svc.reboot(OTHER);
        assert_eq!(error_code(&rig.events()).as_deref(), Some("busy"), "no reboot mid-mutation");
        assert!(!rig.calls().contains(&"reboot".to_string()));

        release_tx.send(()).unwrap();
        worker.join().unwrap();
        let events = rig.events();
        assert_eq!(error_code(&events), None);
        assert_eq!(last_state(&events).busy, None, "busy cleared at the end");
    }

    #[test]
    fn the_busy_marker_names_the_setting_while_it_applies() {
        let rig = Rig::new();
        rig.svc.set(CONN, "uart", "on");
        let busy: Vec<Option<String>> = rig
            .events()
            .iter()
            .filter_map(|e| match e {
                BridgeEvent::DeviceSettings { busy, .. } => Some(busy.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(busy, vec![Some("uart".to_string()), None]);
    }

    #[test]
    fn reboot_needs_the_session_and_emits_device_rebooting_with_no_confirm() {
        let rig = Rig::new();
        rig.svc.reboot(CONN);
        assert!(matches!(rig.events().as_slice(), [BridgeEvent::DeviceRebooting {}]));
        assert_eq!(rig.calls(), vec!["reboot"]);
    }

    #[test]
    fn an_unwritable_store_refuses_every_set_without_touching_the_system() {
        let rig = Rig::new();
        lock(&rig.svc.persisted).persistable = false;
        rig.svc.set(CONN, "ssh", "off");
        assert_eq!(error_code(&rig.events()).as_deref(), Some("persist_failed"));
        assert!(rig.calls().is_empty());
    }
}
