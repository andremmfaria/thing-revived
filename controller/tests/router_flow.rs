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
use controller_agent::ports::{Backlight, EventSink, TimezoneCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
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
fn user_bootstrap_get_replies_unavailable_to_the_requester_only_without_a_service() {
    let r = rig();
    let broadcast = run(&r, r#"{"type":"user_bootstrap_get"}"#);
    assert!(broadcast.is_empty(), "a user_* reply must not be broadcast: {broadcast:?}");
    let targeted = r.sink.take_targeted();
    assert!(
        matches!(&targeted[..], [(c, BridgeEvent::UserError { context, code, .. })] if *c == CONN && context == "user_bootstrap_get" && code == "unavailable"),
        "{targeted:?}"
    );
}

#[test]
fn the_removed_bootstrap_commands_do_not_parse() {
    for json in [r#"{"type":"user_bootstrap_reveal"}"#, r#"{"type":"user_bootstrap_ack","last4":"abcd"}"#] {
        assert!(serde_json::from_str::<ClientCommand>(json).is_err(), "{json}");
    }
}

#[test]
fn every_phase_2_command_replies_not_implemented_to_the_requester_only() {
    let r = rig();
    for (cmd, context) in [
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
    ] {
        let broadcast = run(&r, cmd);
        assert!(broadcast.is_empty(), "{cmd}: a user_* reply must not be broadcast: {broadcast:?}");
        let targeted = r.sink.take_targeted();
        assert!(
            matches!(&targeted[..], [(c, BridgeEvent::UserError { context: x, code, message })]
                if *c == CONN && x == context && code == "not_implemented" && !message.contains("hunter2")),
            "{cmd}: {targeted:?}"
        );
    }
}
