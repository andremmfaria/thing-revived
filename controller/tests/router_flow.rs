//! Router flow with fake ports: a command goes in, wire events come out.
//! No hardware, no sockets: Bluetooth is absent (as when BlueZ is down) and
//! Wi-Fi has no adapter.

use controller_agent::app::error::AppError;
use controller_agent::app::hub::{ConnId, RecordingSink, TargetedSink};
use controller_agent::app::router::Router;
use controller_agent::adapters::unavailable_bluetooth::UnavailableBluetooth;
use controller_agent::domain::bluetooth::AgentEvent;
use controller_agent::services::bluetooth::BluetoothService;
use controller_agent::domain::brightness::validate;
use controller_agent::domain::settings::Settings;
use controller_agent::domain::device_settings::{DeviceSettings, Loaded, Setting, UsbRole, UsbRoleStatus};
use controller_agent::ports::{Backlight, DeviceCtl, DeviceSettingsStore, EventSink, TimezoneCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
use controller_agent::services::accounts::testing::{Rig as AccountsRig, ADMIN_PASSWORD};
use controller_agent::services::device_settings::DeviceSettingsService;
use controller_agent::services::display::DisplayService;
use controller_agent::services::settings::testing::MemStore;
use controller_agent::services::settings::SettingsHolder;
use controller_agent::services::timezone::TimezoneService;
use controller_agent::services::wifi::{Wifi, WifiEvent};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

struct FakeBacklight(Mutex<u32>);

impl Backlight for FakeBacklight {
    fn max(&self) -> Result<u32, AppError> {
        Ok(160)
    }
    fn get(&self) -> Result<u32, AppError> {
        Ok(*self.0.lock().unwrap())
    }
    fn set(&self, v: u32) -> Result<(), AppError> {
        validate(v, 160)?;
        *self.0.lock().unwrap() = v;
        Ok(())
    }
}

struct FakeTz(Mutex<String>);

impl TimezoneCtl for FakeTz {
    fn list(&self) -> Vec<String> {
        vec!["Europe/Lisbon".into(), "UTC".into()]
    }
    fn get(&self) -> Result<String, AppError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set(&self, tz: &str) -> Result<(), AppError> {
        *self.0.lock().unwrap() = tz.to_string();
        Ok(())
    }
}

/// A fake system: live state in maps, every call recorded in order.
struct FakeCtl {
    live: Mutex<std::collections::HashMap<&'static str, bool>>,
    role: Mutex<UsbRole>,
    calls: Mutex<Vec<String>>,
}

impl FakeCtl {
    fn new() -> Arc<FakeCtl> {
        Arc::new(FakeCtl {
            live: Mutex::new([("ssh", true), ("adb", false), ("uart", false)].into_iter().collect()),
            role: Mutex::new(UsbRole::Device),
            calls: Mutex::new(vec![]),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
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
        self.calls.lock().unwrap().push(format!("live {} {}", setting.as_str(), if on { "on" } else { "off" }));
        self.live.lock().unwrap().insert(setting.as_str(), on);
        Ok(())
    }
    fn to_host(&self) -> Result<(), AppError> {
        self.calls.lock().unwrap().push("to_host".into());
        *self.role.lock().unwrap() = UsbRole::Host;
        Ok(())
    }
    fn to_device(&self) -> Result<(), AppError> {
        self.calls.lock().unwrap().push("to_device".into());
        *self.role.lock().unwrap() = UsbRole::Device;
        Ok(())
    }
    fn persist_boot_role(&self, role: UsbRole) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("boot-role {}", role.as_str()));
        Ok(())
    }
    fn reboot(&self) -> Result<(), AppError> {
        self.calls.lock().unwrap().push("reboot".into());
        Ok(())
    }
}

/// Keeps what was saved, in memory.
struct MemSettings {
    saved: Mutex<Vec<DeviceSettings>>,
}

impl DeviceSettingsStore for MemSettings {
    fn load(&self) -> Loaded {
        unreachable!("the router test hands the service its loaded values")
    }
    fn save(&self, _image_id: &str, settings: &DeviceSettings) -> Result<(), AppError> {
        self.saved.lock().unwrap().push(*settings);
        Ok(())
    }
}

struct Rig {
    router: Arc<Router>,
    sink: Arc<RecordingSink>,
    store: Arc<MemStore>,
    wifi_rx: std::sync::mpsc::Receiver<WifiEvent>,
}

fn rig() -> Rig {
    let sink = Arc::new(RecordingSink::new());
    let dyn_sink: Arc<dyn EventSink> = sink.clone();
    let store = Arc::new(MemStore { initial: Settings::default(), ..MemStore::default() });
    let settings = Arc::new(SettingsHolder::load(store.clone()));
    let display = DisplayService::new(Arc::new(FakeBacklight(Mutex::new(10))), settings.clone(), dyn_sink.clone());
    let timezone = TimezoneService::new(Arc::new(FakeTz(Mutex::new("UTC".into()))), settings, dyn_sink.clone());
    let (bt_tx, _bt_rx) = channel::<AgentEvent>();
    let (wifi_tx, wifi_rx) = channel::<WifiEvent>();
    let router = Arc::new(Router::new(
        Arc::new(BluetoothService::new(Arc::new(UnavailableBluetooth), dyn_sink, bt_tx)),
        Arc::new(Wifi::new(wifi_tx)),
        display,
        timezone,
    ));
    let targeted: Arc<dyn TargetedSink> = sink.clone();
    router.attach_replies(targeted);
    Rig { router, sink, store, wifi_rx }
}

/// A router with the real accounts service and the device settings service
/// wired over fakes, their replies landing in the same recording sink, so a
/// `device_settings_set` is gated by a real unlock session exactly as in `run`.
fn rig_with_device_settings() -> (Rig, AccountsRig, Arc<FakeCtl>, Arc<MemSettings>) {
    let r = rig();
    let accounts = AccountsRig::with_sink(&Default::default(), r.sink.clone(), r.sink.clone());
    r.router.attach_accounts(accounts.service.clone());
    let ctl = FakeCtl::new();
    let store = Arc::new(MemSettings { saved: Mutex::new(vec![]) });
    let loaded = Loaded {
        settings: DeviceSettings::new(UsbRole::Device, true, false, false),
        image_id: "img".into(),
        write: false,
        quarantine: false,
        persistable: true,
        notes: vec![],
    };
    let dyn_sink: Arc<dyn EventSink> = r.sink.clone();
    r.router.attach_device_settings(Arc::new(DeviceSettingsService::new(
        store.clone(),
        ctl.clone(),
        accounts.service.admin_gate(),
        dyn_sink,
        loaded,
        6,
    )));
    (r, accounts, ctl, store)
}

const CONN: ConnId = ConnId(7);

fn run(rig: &Rig, json: &str) -> Vec<BridgeEvent> {
    let cmd: ClientCommand = serde_json::from_str(json).unwrap();
    rig.router.execute(CONN, cmd);
    rig.sink.take()
}

#[test]
fn brightness_set_persists_and_reports_and_a_bad_value_does_neither() {
    let r = rig();
    let ev = run(&r, r#"{"type":"brightness_set","value":90}"#);
    assert!(matches!(ev[..], [BridgeEvent::Brightness { value: 90, max: 160 }]));
    assert_eq!(r.store.saved.lock().unwrap().len(), 1);

    let ev = run(&r, r#"{"type":"brightness_set","value":999}"#);
    assert!(matches!(&ev[..], [BridgeEvent::SettingsError { context, .. }] if context == "brightness_set"));
    assert_eq!(r.store.saved.lock().unwrap().len(), 1, "failed set must not persist");
}

#[test]
fn timezone_commands_flow_through_the_service() {
    let r = rig();
    assert!(matches!(&run(&r, r#"{"type":"timezone_list_get"}"#)[..], [BridgeEvent::TimezoneList { zones }] if zones.len() == 2));
    assert!(matches!(&run(&r, r#"{"type":"timezone_set","value":"Europe/Lisbon"}"#)[..], [BridgeEvent::Timezone { value }] if value == "Europe/Lisbon"));
    assert!(matches!(&run(&r, r#"{"type":"timezone_get"}"#)[..], [BridgeEvent::Timezone { value }] if value == "Europe/Lisbon"));
    assert!(matches!(&run(&r, r#"{"type":"timezone_set","value":"Mars/Base"}"#)[..], [BridgeEvent::SettingsError { .. }]));
}

#[test]
fn every_bluetooth_command_without_bluez_replies_bt_error_unavailable() {
    let r = rig();
    for (cmd, context) in [
        (r#"{"type":"bt_power_get"}"#, "bt_power_get"),
        (r#"{"type":"bt_set_power","powered":true}"#, "bt_set_power"),
        (r#"{"type":"bt_scan_start"}"#, "bt_scan_start"),
        (r#"{"type":"bt_scan_stop"}"#, "bt_scan_stop"),
        (r#"{"type":"bt_pair","address":"AA:BB:CC:DD:EE:FF"}"#, "bt_pair"),
        (r#"{"type":"bt_paired_list_get"}"#, "bt_paired_list_get"),
        (r#"{"type":"bt_connect","address":"AA:BB:CC:DD:EE:FF"}"#, "bt_connect"),
        (r#"{"type":"bt_disconnect","address":"AA:BB:CC:DD:EE:FF"}"#, "bt_disconnect"),
        (r#"{"type":"bt_forget","address":"AA:BB:CC:DD:EE:FF"}"#, "bt_forget"),
    ] {
        let ev = run(&r, cmd);
        assert!(
            matches!(&ev[..], [BridgeEvent::BtError { context: c, message }] if c == context && message == "Bluetooth unavailable"),
            "{cmd}: {ev:?}"
        );
    }
    // A confirmation with nothing to confirm is only logged, never answered.
    assert!(run(&r, r#"{"type":"bt_pairing_confirm","accept":true}"#).is_empty());
}

#[test]
fn wifi_get_without_an_adapter_announces_unavailable() {
    let r = rig();
    run(&r, r#"{"type":"wifi_get"}"#);
    // Wi-Fi events travel on their own channel (mapped to the wire by `run`).
    let events: Vec<BridgeEvent> = r.wifi_rx.try_iter().map(BridgeEvent::from).collect();
    assert!(matches!(&events[..], [BridgeEvent::WifiAvailable { available: false, interface: None }]), "{events:?}");
}

#[test]
fn wifi_scan_without_an_adapter_reports_a_wifi_error() {
    let r = rig();
    run(&r, r#"{"type":"wifi_scan"}"#);
    let events: Vec<BridgeEvent> = r.wifi_rx.try_iter().map(BridgeEvent::from).collect();
    assert!(matches!(&events[..], [BridgeEvent::WifiError { context, message }] if context == "wifi_scan" && message == "No Wi-Fi adapter"), "{events:?}");
}

#[test]
fn user_bootstrap_get_replies_internal_to_the_requester_only_without_a_service() {
    let r = rig();
    let broadcast = run(&r, r#"{"type":"user_bootstrap_get"}"#);
    assert!(broadcast.is_empty(), "a user_* reply must not be broadcast: {broadcast:?}");
    let targeted = r.sink.take_targeted();
    assert!(
        matches!(&targeted[..], [(c, BridgeEvent::UserError { context, code, .. })] if *c == CONN && context == "user_bootstrap_get" && code == "internal"),
        "{targeted:?}"
    );
}

#[test]
fn the_removed_bootstrap_commands_do_not_parse() {
    for json in [r#"{"type":"user_bootstrap_reveal"}"#, r#"{"type":"user_bootstrap_ack","last4":"abcd"}"#] {
        assert!(serde_json::from_str::<ClientCommand>(json).is_err(), "{json}");
    }
}

const PHASE_2: &[(&str, &str)] = &[
        (r#"{"type":"user_unlock","username":"ana","password":"hunter2hunter2"}"#, "user_unlock"),
        (r#"{"type":"user_lock_session"}"#, "user_lock_session"),
        (r#"{"type":"user_list_get"}"#, "user_list_get"),
        (r#"{"type":"user_add","name":"ana"}"#, "user_add"),
        (r#"{"type":"user_remove","name":"ana"}"#, "user_remove"),
        (r#"{"type":"user_set_locked","name":"ana","locked":true}"#, "user_set_locked"),
        (r#"{"type":"user_set_password","name":"ana"}"#, "user_set_password"),
        (r#"{"type":"group_list_get"}"#, "group_list_get"),
        (r#"{"type":"user_group_add","name":"ana","group":"adm"}"#, "user_group_add"),
        (r#"{"type":"user_group_remove","name":"ana","group":"adm"}"#, "user_group_remove"),
        (r#"{"type":"group_add","name":"crew"}"#, "group_add"),
        (r#"{"type":"group_remove","name":"crew"}"#, "group_remove"),
        (r#"{"type":"ssh_key_list_get","name":"ana"}"#, "ssh_key_list_get"),
        (r#"{"type":"ssh_key_add","name":"ana","key":"ssh-ed25519 AAAA"}"#, "ssh_key_add"),
        (r#"{"type":"ssh_key_remove","name":"ana","fingerprint":"SHA256:x"}"#, "ssh_key_remove"),
];

#[test]
fn every_phase_2_command_replies_internal_to_the_requester_only_without_the_accounts_service() {
    let r = rig();
    for (cmd, context) in PHASE_2 {
        let broadcast = run(&r, cmd);
        assert!(broadcast.is_empty(), "{cmd}: a user_* reply must not be broadcast: {broadcast:?}");
        let targeted = r.sink.take_targeted();
        assert!(
            matches!(&targeted[..], [(c, BridgeEvent::UserError { context: x, code, message })]
                if *c == CONN && x == context && code == "internal" && !message.contains("hunter2")),
            "{cmd}: {targeted:?}"
        );
    }
}

/// A router over the real accounts service (fake ports) whose replies land in
/// the same recording sink.
fn rig_with_accounts() -> (Rig, AccountsRig) {
    let r = rig();
    let accounts = AccountsRig::with_sink(&Default::default(), r.sink.clone(), r.sink.clone());
    r.router.attach_accounts(accounts.service.clone());
    (r, accounts)
}

#[test]
fn a_locked_connection_gets_not_unlocked_for_every_gated_command_and_nothing_is_broadcast() {
    let (r, accounts) = rig_with_accounts();
    for (cmd, context) in PHASE_2 {
        if matches!(*context, "user_unlock" | "user_lock_session" | "user_list_get") {
            continue;
        }
        let broadcast = run(&r, cmd);
        assert!(broadcast.is_empty(), "{cmd}: {broadcast:?}");
        let targeted = r.sink.take_targeted();
        assert!(
            matches!(&targeted[..], [(c, BridgeEvent::UserError { context: x, code, .. })] if *c == CONN && x == context && code == "not_unlocked"),
            "{cmd}: {targeted:?}"
        );
    }
    assert!(accounts.sys.calls().is_empty(), "no tool ran for a locked connection");
}

#[test]
fn unlock_then_list_add_and_lock_flow_through_the_router() {
    let (r, accounts) = rig_with_accounts();
    let wrong = run(&r, r#"{"type":"user_unlock","username":"kiosk","password":"hunter2hunter2"}"#);
    assert!(wrong.is_empty());
    assert!(matches!(&r.sink.take_targeted()[..], [(c, BridgeEvent::UserError { code, .. })] if *c == CONN && code == "bad_password"));
    let json = format!(r#"{{"type":"user_unlock","username":"kiosk","password":"{ADMIN_PASSWORD}"}}"#);
    run(&r, &json);
    assert!(matches!(&r.sink.take_targeted()[..], [(c, BridgeEvent::UserSession { unlocked: true, .. })] if *c == CONN));
    run(&r, r#"{"type":"user_add","name":"carol","password":"","admin":false}"#);
    let got = r.sink.take_targeted();
    assert!(matches!(&got[..], [(_, BridgeEvent::UserSecret { name, .. }), (_, BridgeEvent::UserResult { success: true, .. })] if name == "carol"), "{got:?}");
    run(&r, r#"{"type":"user_list_get"}"#);
    assert!(matches!(&r.sink.take_targeted()[..], [(_, BridgeEvent::UserList { users })] if users.iter().any(|u| u.name == "carol")));
    run(&r, r#"{"type":"user_lock_session"}"#);
    assert!(matches!(&r.sink.take_targeted()[..], [(_, BridgeEvent::UserSession { unlocked: false, .. })]));
    run(&r, r#"{"type":"group_add","name":"crew2"}"#);
    assert!(matches!(&r.sink.take_targeted()[..], [(_, BridgeEvent::UserError { code, .. })] if code == "not_unlocked"));
    assert!(!accounts.sys.calls().iter().any(|c| c.contains("crew2")));
}

#[test]
fn closing_a_connection_ends_its_unlock_session() {
    let (r, _accounts) = rig_with_accounts();
    let json = format!(r#"{{"type":"user_unlock","username":"kiosk","password":"{ADMIN_PASSWORD}"}}"#);
    run(&r, &json);
    r.sink.take_targeted();
    r.router.connection_closed(CONN);
    run(&r, r#"{"type":"group_add","name":"crew2"}"#);
    assert!(matches!(&r.sink.take_targeted()[..], [(_, BridgeEvent::UserError { code, .. })] if code == "not_unlocked"));
}

#[test]
fn device_settings_commands_without_a_service_answer_unavailable() {
    let sink = Arc::new(RecordingSink::new());
    let dyn_sink: Arc<dyn EventSink> = sink.clone();
    let store = Arc::new(MemStore { initial: Settings::default(), ..MemStore::default() });
    let settings = Arc::new(SettingsHolder::load(store));
    let display = DisplayService::new(Arc::new(FakeBacklight(Mutex::new(10))), settings.clone(), dyn_sink.clone());
    let timezone = TimezoneService::new(Arc::new(FakeTz(Mutex::new("UTC".into()))), settings, dyn_sink.clone());
    let (bt_tx, _bt_rx) = channel::<AgentEvent>();
    let (wifi_tx, _wifi_rx) = channel::<WifiEvent>();
    let router = Arc::new(Router::new(
        Arc::new(BluetoothService::new(Arc::new(UnavailableBluetooth), dyn_sink, bt_tx)),
        Arc::new(Wifi::new(wifi_tx)),
        display,
        timezone,
    ));
    let targeted: Arc<dyn TargetedSink> = sink.clone();
    router.attach_replies(targeted);
    for (json, context) in [
        (r#"{"type":"device_settings_get"}"#, "device_settings_get"),
        (r#"{"type":"device_settings_set","setting":"ssh","value":"off"}"#, "device_settings_set"),
        (r#"{"type":"device_reboot"}"#, "device_reboot"),
    ] {
        router.execute(CONN, serde_json::from_str(json).unwrap());
        let t = sink.take_targeted();
        assert!(
            matches!(&t[..], [(c, BridgeEvent::DeviceSettingsError { context: x, code, .. })] if *c == CONN && x == context && code == "unavailable"),
            "{json}: {t:?}"
        );
    }
}

#[test]
fn device_settings_get_reports_live_and_persisted_without_an_unlocked_session() {
    let (r, _accounts, ctl, _store) = rig_with_device_settings();
    let ev = run(&r, r#"{"type":"device_settings_get"}"#);
    assert!(
        matches!(&ev[..], [BridgeEvent::DeviceSettings { mode, ssh, adb, uart, adb_blocked: None, busy: None, .. }]
            if mode == "device" && ssh == "on" && adb == "off" && uart == "off"),
        "{ev:?}"
    );
    assert!(ctl.calls().is_empty());
}

#[test]
fn device_settings_set_without_an_unlocked_session_is_not_unlocked_and_never_touches_the_system() {
    let (r, _accounts, ctl, store) = rig_with_device_settings();
    let ev = run(&r, r#"{"type":"device_settings_set","setting":"adb","value":"on"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceSettingsError { context, code, .. }] if context == "device_settings_set" && code == "not_unlocked"), "{ev:?}");
    let ev = run(&r, r#"{"type":"device_reboot"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceSettingsError { context, code, .. }] if context == "device_reboot" && code == "not_unlocked"), "{ev:?}");
    assert!(ctl.calls().is_empty());
    assert!(store.saved.lock().unwrap().is_empty());
}

#[test]
fn unlock_then_set_applies_persists_and_get_sees_it_with_no_lock_side_effects() {
    let (r, accounts, ctl, store) = rig_with_device_settings();
    accounts.unlock(CONN);
    let ev = run(&r, r#"{"type":"device_settings_set","setting":"adb","value":"on"}"#);
    let last = ev.last().expect("a state event");
    assert!(matches!(last, BridgeEvent::DeviceSettings { adb, persisted, busy: None, .. } if adb == "on" && persisted.adb == "on"), "{ev:?}");
    assert_eq!(ctl.calls(), vec!["live adb on".to_string()]);
    assert_eq!(store.saved.lock().unwrap().len(), 1);
    let ev = run(&r, r#"{"type":"device_settings_get"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceSettings { adb, .. }] if adb == "on"), "{ev:?}");
    // Locking the session does not turn anything off: adb and uart persist.
    run(&r, r#"{"type":"user_lock_session"}"#);
    assert_eq!(ctl.calls(), vec!["live adb on".to_string()], "no auto disable on lock");
    let ev = run(&r, r#"{"type":"device_settings_get"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceSettings { adb, .. }] if adb == "on"), "{ev:?}");
}

#[test]
fn host_mode_forces_adb_off_and_then_blocks_adb_on_end_to_end() {
    let (r, accounts, ctl, store) = rig_with_device_settings();
    accounts.unlock(CONN);
    run(&r, r#"{"type":"device_settings_set","setting":"adb","value":"on"}"#);
    let ev = run(&r, r#"{"type":"device_settings_set","setting":"mode","value":"host"}"#);
    let last = ev.last().unwrap();
    assert!(
        matches!(last, BridgeEvent::DeviceSettings { mode, adb, adb_blocked: Some(b), persisted, .. }
            if mode == "host" && adb == "off" && b == "mode_host" && persisted.mode == "host" && persisted.adb == "off"),
        "{ev:?}"
    );
    assert_eq!(
        ctl.calls(),
        vec!["live adb on", "live adb off", "boot-role host", "to_host"].into_iter().map(String::from).collect::<Vec<_>>()
    );
    // One atomic write persisted host together with adb off.
    let saved = store.saved.lock().unwrap().clone();
    assert_eq!((saved[1].mode(), saved[1].adb()), (UsbRole::Host, false));
    let ev = run(&r, r#"{"type":"device_settings_set","setting":"adb","value":"on"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceSettingsError { code, .. }] if code == "adb_blocked_host"), "{ev:?}");
}

#[test]
fn bad_setting_and_value_never_reach_the_system() {
    let (r, accounts, ctl, store) = rig_with_device_settings();
    accounts.unlock(CONN);
    for (setting, value, code) in [("Mode", "host", "invalid_setting"), ("ssh", "yes", "invalid_value"), ("mode", "on", "invalid_value")] {
        let ev = run(&r, &format!(r#"{{"type":"device_settings_set","setting":"{setting}","value":"{value}"}}"#));
        assert!(matches!(&ev[..], [BridgeEvent::DeviceSettingsError { code: c, .. }] if c == code), "{setting}={value}: {ev:?}");
    }
    assert!(ctl.calls().is_empty());
    assert!(store.saved.lock().unwrap().is_empty());
}

#[test]
fn device_reboot_after_unlock_answers_rebooting() {
    let (r, accounts, ctl, _store) = rig_with_device_settings();
    accounts.unlock(CONN);
    let ev = run(&r, r#"{"type":"device_reboot"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DeviceRebooting {}]), "{ev:?}");
    assert_eq!(ctl.calls(), vec!["reboot".to_string()]);
}
