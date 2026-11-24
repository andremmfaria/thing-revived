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
use controller_agent::domain::debug_access::InFlight;
use controller_agent::domain::usb_role::{parse_status, UsbRole, UsbRoleStatus};
use controller_agent::ports::{Backlight, DebugAccessCtl, EventSink, TimezoneCtl, UsbRoleCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
use controller_agent::services::accounts::testing::{Rig as AccountsRig, ADMIN_PASSWORD};
use controller_agent::services::debug_access::DebugAccessService;
use controller_agent::services::display::DisplayService;
use controller_agent::services::settings::testing::MemStore;
use controller_agent::services::settings::SettingsHolder;
use controller_agent::services::timezone::TimezoneService;
use controller_agent::services::usb_role::UsbRoleService;
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

/// Records the commands it was given; `status` parses a canned tool output
/// that `persist` rewrites, like the real tool would.
struct FakeUsb {
    out: Mutex<String>,
    calls: Mutex<Vec<String>>,
}

impl UsbRoleCtl for FakeUsb {
    fn status(&self) -> Result<UsbRoleStatus, AppError> {
        Ok(parse_status(&self.out.lock().unwrap()))
    }
    fn persist(&self, role: UsbRole) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("persist {}", role.as_str()));
        *self.out.lock().unwrap() = format!("role: device\nrevert: none\nboot role: {}\n", role.as_str());
        Ok(())
    }
    fn reboot(&self) -> Result<(), AppError> {
        self.calls.lock().unwrap().push("reboot".into());
        Ok(())
    }
    fn adb_forced(&self) -> bool {
        false
    }
}

/// Records every call; `status` and `build_default` answer from the maps
/// below, mutated as `enable`/`disable` run, like the real adapter's
/// systemd units would.
struct FakeDebugCtl {
    status: Mutex<std::collections::HashMap<&'static str, bool>>,
    build_default: Mutex<std::collections::HashMap<&'static str, bool>>,
    calls: Mutex<Vec<String>>,
}

impl FakeDebugCtl {
    fn new() -> Arc<FakeDebugCtl> {
        Arc::new(FakeDebugCtl {
            status: Mutex::new([("adb", false), ("uart", false)].into_iter().collect()),
            build_default: Mutex::new([("adb", false), ("uart", false)].into_iter().collect()),
            calls: Mutex::new(vec![]),
        })
    }
}

impl DebugAccessCtl for FakeDebugCtl {
    fn status(&self, surface: controller_agent::domain::debug_access::Surface) -> Result<bool, AppError> {
        Ok(*self.status.lock().unwrap().get(surface.as_str()).unwrap_or(&false))
    }
    fn enable(&self, surface: controller_agent::domain::debug_access::Surface) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("enable {}", surface.as_str()));
        self.status.lock().unwrap().insert(surface.as_str(), true);
        Ok(())
    }
    fn disable(&self, surface: controller_agent::domain::debug_access::Surface) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("disable {}", surface.as_str()));
        self.status.lock().unwrap().insert(surface.as_str(), false);
        Ok(())
    }
    fn build_default(&self, surface: controller_agent::domain::debug_access::Surface) -> Result<bool, AppError> {
        Ok(*self.build_default.lock().unwrap().get(surface.as_str()).unwrap_or(&false))
    }
}

struct Rig {
    router: Arc<Router>,
    sink: Arc<RecordingSink>,
    usb: Arc<FakeUsb>,
    store: Arc<MemStore>,
    wifi_rx: std::sync::mpsc::Receiver<WifiEvent>,
}

fn rig() -> Rig {
    let sink = Arc::new(RecordingSink::new());
    let dyn_sink: Arc<dyn EventSink> = sink.clone();
    let dyn_sink2 = dyn_sink.clone();
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
    let usb = Arc::new(FakeUsb { out: Mutex::new("role: device\nrevert: none\nboot role: none\n".into()), calls: Mutex::new(vec![]) });
    router.attach_usb_role(Arc::new(UsbRoleService::new(usb.clone(), dyn_sink2, Arc::new(InFlight::new()))));
    Rig { router, sink, store, usb, wifi_rx }
}

/// A router with both the real accounts service and debug-access wired
/// over fakes, their replies landing in the same recording sink, so a
/// `debug_set` can be gated by a real unlock session exactly as in `run`.
fn rig_with_debug_access() -> (Rig, AccountsRig, Arc<FakeDebugCtl>) {
    let r = rig();
    let accounts = AccountsRig::with_sink(&Default::default(), r.sink.clone(), r.sink.clone());
    r.router.attach_accounts(accounts.service.clone());
    let debug_ctl = FakeDebugCtl::new();
    let dyn_debug_ctl: Arc<dyn DebugAccessCtl> = debug_ctl.clone();
    let dyn_usb: Arc<dyn UsbRoleCtl> = r.usb.clone();
    let dyn_sink: Arc<dyn EventSink> = r.sink.clone();
    let debug_access = Arc::new(DebugAccessService::new(
        dyn_debug_ctl,
        dyn_usb,
        accounts.service.admin_gate(),
        dyn_sink,
        Arc::new(InFlight::new()),
    ));
    // Production code (`lib.rs`) always does this right after construction;
    // the auto-disable-on-session-end hook never fires without it.
    debug_access.register_auto_disable();
    r.router.attach_debug_access(debug_access);
    (r, accounts, debug_ctl)
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
fn usb_role_get_set_and_reboot_flow_through_the_service() {
    let r = rig();
    let ev = run(&r, r#"{"type":"usb_role_get"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::UsbRole { role: Some(role), boot_role: None, adb_forced: false }] if role == "device"), "{ev:?}");

    let ev = run(&r, r#"{"type":"usb_role_set","role":"host"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::UsbRole { boot_role: Some(b), .. }] if b == "host"), "{ev:?}");

    let ev = run(&r, r#"{"type":"usb_role_reboot"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::UsbRoleRebooting {}]), "{ev:?}");
    assert_eq!(r.usb.calls.lock().unwrap().clone(), vec!["persist host".to_string(), "reboot".to_string()]);
}

#[test]
fn usb_role_set_with_a_bad_value_never_reaches_the_tool() {
    let r = rig();
    for bad in ["Host", "host;reboot", "", "to-host", "clear"] {
        let ev = run(&r, &format!(r#"{{"type":"usb_role_set","role":"{bad}"}}"#));
        assert!(matches!(&ev[..], [BridgeEvent::UsbRoleError { context, code, .. }] if context == "usb_role_set" && code == "invalid_role"), "{bad}: {ev:?}");
    }
    assert!(r.usb.calls.lock().unwrap().is_empty());
}

#[test]
fn usb_role_commands_without_a_service_answer_unavailable() {
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
    for json in [r#"{"type":"usb_role_get"}"#, r#"{"type":"usb_role_set","role":"host"}"#, r#"{"type":"usb_role_reboot"}"#] {
        router.execute(CONN, serde_json::from_str(json).unwrap());
        let t = sink.take_targeted();
        assert!(matches!(&t[..], [(c, BridgeEvent::UsbRoleError { code, .. })] if *c == CONN && code == "unavailable"), "{json}: {t:?}");
    }
    for json in [r#"{"type":"debug_get"}"#, r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"adb"}"#] {
        router.execute(CONN, serde_json::from_str(json).unwrap());
        let t = sink.take_targeted();
        assert!(matches!(&t[..], [(c, BridgeEvent::DebugError { code, .. })] if *c == CONN && code == "unavailable"), "{json}: {t:?}");
    }
}

#[test]
fn debug_get_reports_live_status_without_an_unlocked_session() {
    let (r, _accounts, _ctl) = rig_with_debug_access();
    let ev = run(&r, r#"{"type":"debug_get"}"#);
    assert!(
        matches!(&ev[..], [BridgeEvent::DebugState { adb: false, uart: false, adb_available: true, blocked: None, .. }]),
        "{ev:?}"
    );
}

#[test]
fn debug_set_without_an_unlocked_session_is_not_unlocked_and_never_touches_the_adapter() {
    let (r, _accounts, ctl) = rig_with_debug_access();
    let ev = run(&r, r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"adb"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DebugError { context, code, .. }] if context == "debug_set" && code == "not_unlocked"), "{ev:?}");
    assert!(ctl.calls.lock().unwrap().is_empty());
}

#[test]
fn unlock_then_debug_set_enables_and_debug_get_sees_it() {
    let (r, accounts, ctl) = rig_with_debug_access();
    accounts.unlock(CONN);
    let ev = run(&r, r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"adb"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DebugState { adb: true, .. }]), "{ev:?}");
    assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["enable adb".to_string()]);
    let ev = run(&r, r#"{"type":"debug_get"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DebugState { adb: true, .. }]), "{ev:?}");
    // Locking the session reverts what it enabled at runtime. `run` only
    // returns broadcast events; `user_lock_session`'s own targeted reply
    // is left in the sink, only the broadcast `debug_state` is checked.
    let ev = run(&r, r#"{"type":"user_lock_session"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DebugState { adb: false, .. }]), "{ev:?}");
    assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["enable adb".to_string(), "disable adb".to_string()]);
}

#[test]
fn debug_set_with_a_bad_confirm_never_reaches_the_adapter() {
    let (r, accounts, ctl) = rig_with_debug_access();
    accounts.unlock(CONN);
    let ev = run(&r, r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"uart"}"#);
    assert!(matches!(&ev[..], [BridgeEvent::DebugError { code, .. }] if code == "confirm_required"), "{ev:?}");
    assert!(ctl.calls.lock().unwrap().is_empty());
}
