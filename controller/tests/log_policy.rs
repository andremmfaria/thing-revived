//! Log policy: nothing secret from a `user_*` event reaches the journal,
//! through either the broadcast or the targeted path. A capturing `log`
//! backend records every line the hub writes at any level.

use controller_agent::app::hub::{EventHub, TargetedSink};
use controller_agent::ports::EventSink;
use controller_agent::protocol::{BridgeEvent, Secret};
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

fn secret(s: &str) -> Secret {
    serde_json::from_str(&format!("\"{s}\"")).unwrap()
}

/// Every user-related event the protocol has today, carrying sentinels in
/// every field. Add new `user_*`, `group_*` and `ssh_key_*` variants here.
fn user_events() -> Vec<(BridgeEvent, &'static str)> {
    vec![
        (
            BridgeEvent::UserBootstrap { pending: true, username: USERNAME.into(), acknowledged: false, expires_in_secs: Some(321), storage_ok: true },
            "user_bootstrap",
        ),
        (
            BridgeEvent::UserBootstrapSecret { username: USERNAME.into(), password: secret(PASSWORD), expires_in_secs: 654 },
            "user_bootstrap_secret",
        ),
        (BridgeEvent::UserError { context: CONTEXT.into(), code: "bad_ack".into(), message: MESSAGE.into() }, "user_error"),
    ]
}

fn assert_clean(log: &str, ty: &str) {
    assert!(log.contains(ty), "type {ty} should be logged: {log:?}");
    for leaked in [PASSWORD, USERNAME, CONTEXT, MESSAGE, "321", "654"] {
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
        let log = captured_during(|| hub.emit_to(conn, event.clone()));
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
        assert_clean(&captured_during(|| hub.emit_to(gone, event.clone())), ty);
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
