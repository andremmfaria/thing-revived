//! Router flow with fake ports: a command goes in, wire events come out.
//! No hardware, no sockets: Bluetooth is absent (as when BlueZ is down) and
//! Wi-Fi has no adapter.

use controller_agent::app::error::AppError;
use controller_agent::app::hub::RecordingSink;
use controller_agent::app::router::Router;
use controller_agent::bluetooth;
use controller_agent::domain::brightness::validate;
use controller_agent::domain::settings::Settings;
use controller_agent::ports::{Backlight, EventSink, TimezoneCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
use controller_agent::services::display::DisplayService;
use controller_agent::services::settings::testing::MemStore;
use controller_agent::services::settings::SettingsHolder;
use controller_agent::services::timezone::TimezoneService;
use controller_agent::wifi;
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
    wifi_rx: std::sync::mpsc::Receiver<wifi::WifiEvent>,
}

fn rig() -> Rig {
    let sink = Arc::new(RecordingSink::new());
    let dyn_sink: Arc<dyn EventSink> = sink.clone();
    let store = Arc::new(MemStore { initial: Settings::default(), ..MemStore::default() });
    let settings = Arc::new(SettingsHolder::load(store.clone()));
    let display = DisplayService::new(Arc::new(FakeBacklight(Mutex::new(10))), settings.clone(), dyn_sink.clone());
    let timezone = TimezoneService::new(Arc::new(FakeTz(Mutex::new("UTC".into()))), settings, dyn_sink.clone());
    let (bt_tx, _bt_rx) = channel::<bluetooth::AgentEvent>();
    let (wifi_tx, wifi_rx) = channel::<wifi::WifiEvent>();
    let router = Arc::new(Router::new(dyn_sink, None, bt_tx, Arc::new(wifi::Wifi::new(wifi_tx)), display, timezone));
    Rig { router, sink, store, wifi_rx }
}

fn run(rig: &Rig, json: &str) -> Vec<BridgeEvent> {
    let cmd: ClientCommand = serde_json::from_str(json).unwrap();
    rig.router.execute(cmd);
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
