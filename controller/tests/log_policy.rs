//! Log policy: nothing secret from a `user_*` event reaches the journal,
//! through either the broadcast or the targeted path. A capturing `log`
//! backend records every line the hub writes at any level.

use controller_agent::app::hub::{EventHub, TargetedSink};
use controller_agent::ports::EventSink;
use controller_agent::protocol::{BootstrapPhaseWire, BridgeEvent, DeviceSettingsValuesWire, GroupWire, Secret, SshKeyWire, UserWire};
use log::{LevelFilter, Log, Metadata, Record};
use std::sync::mpsc::sync_channel;
use std::sync::{Mutex, Once};

struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }
    fn log(&self, r: &Record) {
        self.0.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
    }
    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));
static INIT: Once = Once::new();
/// Tests share one process-wide logger, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn captured_during(f: impl FnOnce()) -> String {
    INIT.call_once(|| {
        log::set_logger(&CAPTURE).unwrap();
        log::set_max_level(LevelFilter::Trace);
    });
    CAPTURE.0.lock().unwrap().clear();
    f();
    CAPTURE.0.lock().unwrap().join("\n")
}

const PASSWORD: &str = "Zx9-sentinel-password";
const USERNAME: &str = "sentinel-user";
const CONTEXT: &str = "sentinel-context";
const MESSAGE: &str = "sentinel-message";
const HOME: &str = "/sentinel/home";
const FINGERPRINT: &str = "SHA256:sentinel-fingerprint";

fn secret(s: &str) -> Secret {
    serde_json::from_str(&format!("\"{s}\"")).unwrap()
}

/// Every user-related event the protocol has today, carrying sentinels in
/// every field. Add new `user_*`, `group_*` and `ssh_key_*` variants here.
fn user_events() -> Vec<(BridgeEvent, &'static str)> {
    vec![
        (BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Showing, username: USERNAME.into(), storage_ok: true }, "user_bootstrap"),
        (BridgeEvent::UserBootstrapSecret { username: USERNAME.into(), password: secret(PASSWORD) }, "user_bootstrap_secret"),
        (BridgeEvent::UserError { context: CONTEXT.into(), code: "bad_password".into(), message: MESSAGE.into() }, "user_error"),
        (BridgeEvent::UserSession { unlocked: true, expires_in_secs: Some(321), username: Some(USERNAME.into()) }, "user_session"),
        (
            BridgeEvent::UserList {
                users: vec![UserWire {
                    name: USERNAME.into(),
                    uid: 1000,
                    gid: 1000,
                    home: HOME.into(),
                    shell: "/bin/sh".into(),
                    locked: false,
                    admin: true,
                    system: false,
                    protected: false,
                    groups: vec![MESSAGE.into()],
                    key_count: 654,
                }],
            },
            "user_list",
        ),
        (
            BridgeEvent::UserResult { op: "user_add".into(), name: USERNAME.into(), success: false, error: Some(MESSAGE.into()), message: Some(MESSAGE.into()) },
            "user_result",
        ),
        (BridgeEvent::UserSecret { name: USERNAME.into(), password: secret(PASSWORD) }, "user_secret"),
        (
            BridgeEvent::GroupList { groups: vec![GroupWire { name: USERNAME.into(), gid: 1001, system: false, assignable: true, members: vec![MESSAGE.into()] }] },
            "group_list",
        ),
        (
            BridgeEvent::SshKeyList {
                name: USERNAME.into(),
                keys: vec![SshKeyWire { fingerprint: FINGERPRINT.into(), key_type: "ssh-ed25519".into(), comment: MESSAGE.into() }],
            },
            "ssh_key_list",
        ),
    ]
}

fn assert_clean(log: &str, ty: &str) {
    assert!(log.contains(ty), "type {ty} should be logged: {log:?}");
    for leaked in [PASSWORD, USERNAME, CONTEXT, MESSAGE, HOME, FINGERPRINT, "321", "654"] {
        assert!(!log.contains(leaked), "{ty}: {leaked:?} reached the log: {log:?}");
    }
}

#[test]
fn broadcast_of_every_user_event_logs_type_only() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx, rx) = sync_channel::<String>(8);
    hub.register(tx, None);
    for (event, ty) in user_events() {
        let log = captured_during(|| hub.emit(event.clone()));
        assert_clean(&log, ty);
        assert!(rx.try_recv().is_ok(), "{ty} still goes to the client");
    }
}

#[test]
fn targeted_reply_of_every_user_event_logs_type_only() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx, rx) = sync_channel::<String>(8);
    let conn = hub.register(tx, None);
    for (event, ty) in user_events() {
        let log = captured_during(|| { hub.emit_to(conn, event.clone()); });
        assert_clean(&log, ty);
        assert!(rx.try_recv().is_ok(), "{ty} still goes to the client");
    }
}

#[test]
fn a_dropped_targeted_reply_does_not_log_the_payload_either() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx, _rx) = sync_channel::<String>(8);
    let gone = hub.register(tx, None);
    hub.unregister(gone);
    for (event, ty) in user_events() {
        assert_clean(&captured_during(|| { hub.emit_to(gone, event.clone()); }), ty);
    }
}

#[test]
fn the_password_reaches_only_the_requesting_connection() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx_a, rx_a) = sync_channel::<String>(8);
    let (tx_b, rx_b) = sync_channel::<String>(8);
    let a = hub.register(tx_a, None);
    let _b = hub.register(tx_b, None);
    for (event, ty) in user_events() {
        hub.emit_to(a, event);
        let got = rx_a.try_recv().unwrap();
        assert!(got.contains(&format!("\"type\":\"{ty}\"")), "{got}");
        assert!(rx_b.try_recv().is_err(), "{ty} leaked to the other connection");
    }
}

#[test]
fn log_policy_has_no_wildcard_arm() {
    // The compiler enforces exhaustiveness only while no `_ =>` arm exists.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/hub.rs")).unwrap();
    let start = src.find("fn log_policy(").expect("log_policy exists");
    let body = &src[start..];
    let end = body.find("\n}\n").expect("end of log_policy");
    assert!(!body[..end].contains("_ =>"), "log_policy must list every variant explicitly");
}

#[test]
fn device_settings_events_are_logged_in_full_because_they_carry_no_user_data() {
    // The journal is where the plan wants every change visible: four setting
    // values and a stable code, never an account name or a password.
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx, rx) = sync_channel::<String>(8);
    hub.register(tx, None);
    let persisted = DeviceSettingsValuesWire { mode: "device".into(), ssh: "on".into(), adb: "off".into(), uart: "off".into() };
    let state = BridgeEvent::DeviceSettings {
        mode: "host".into(),
        ssh: "on".into(),
        adb: "off".into(),
        uart: "on".into(),
        persisted,
        adb_blocked: Some("mode_host".into()),
        busy: None,
    };
    let log = captured_during(|| hub.emit(state));
    assert!(log.contains("device_settings") && log.contains("mode_host") && log.contains("\"uart\":\"on\""), "{log:?}");
    assert!(rx.try_recv().is_ok());
    let error = BridgeEvent::DeviceSettingsError { context: "device_settings_set".into(), code: "adb_blocked_host".into(), message: "m".into() };
    let log = captured_during(|| hub.emit(error));
    assert!(log.contains("adb_blocked_host"), "{log:?}");
    let log = captured_during(|| hub.emit(BridgeEvent::DeviceRebooting {}));
    assert!(log.contains("device_rebooting"), "{log:?}");
}

#[test]
fn the_unlock_event_with_its_ttl_still_logs_the_type_only() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let hub = EventHub::new();
    let (tx, _rx) = sync_channel::<String>(8);
    hub.register(tx, None);
    let log = captured_during(|| hub.emit(BridgeEvent::UserSession { unlocked: true, expires_in_secs: Some(1800), username: Some(USERNAME.into()) }));
    assert!(log.contains("user_session"), "{log:?}");
    assert!(!log.contains(USERNAME) && !log.contains("1800"), "{log:?}");
}
