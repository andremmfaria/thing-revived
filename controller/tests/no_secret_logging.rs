//! End to end: the generated first-boot password, the acknowledgement proof
//! and a wrong guess never reach a log line, an error string, a `Debug`
//! rendering or any connection other than the requester. A capturing `log`
//! backend records every line any module writes at any level, while real
//! commands run through the router, the user service (over fake ports) and
//! the event hub.

use controller_agent::adapters::unavailable_bluetooth::UnavailableBluetooth;
use controller_agent::app::error::AppError;
use controller_agent::app::hub::{ConnId, EventHub, TargetedSink};
use controller_agent::app::router::Router;
use controller_agent::domain::bluetooth::AgentEvent;
use controller_agent::domain::brightness::validate;
use controller_agent::domain::settings::Settings;
use controller_agent::ports::{Backlight, EventSink, TimezoneCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
use controller_agent::services::bluetooth::BluetoothService;
use controller_agent::services::display::DisplayService;
use controller_agent::services::settings::testing::MemStore;
use controller_agent::services::settings::SettingsHolder;
use controller_agent::services::timezone::TimezoneService;
use controller_agent::services::users::testing::Rig;
use controller_agent::services::wifi::Wifi;
use log::{LevelFilter, Log, Metadata, Record};
use std::sync::mpsc::{channel, sync_channel, Receiver};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }
    fn log(&self, r: &Record) {
        self.0.lock().unwrap().push(format!("{} {}: {}", r.level(), r.target(), r.args()));
    }
    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));
static INIT: Once = Once::new();
/// One process-wide logger, so the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn start_capture() {
    INIT.call_once(|| {
        log::set_logger(&CAPTURE).unwrap();
        log::set_max_level(LevelFilter::Trace);
    });
    CAPTURE.0.lock().unwrap().clear();
}

fn captured() -> String {
    CAPTURE.0.lock().unwrap().join("\n")
}

/// Distinctive on purpose: no log line can contain either by chance.
const PASSWORD: &str = "Qw8Lh3NzVk5Rm4XT";
const LAST4: &str = "m4XT";
const WRONG: &str = "Wr0ng";

struct FakeBacklight;

impl Backlight for FakeBacklight {
    fn max(&self) -> Result<u32, AppError> {
        Ok(160)
    }
    fn get(&self) -> Result<u32, AppError> {
        Ok(10)
    }
    fn set(&self, v: u32) -> Result<(), AppError> {
        validate(v, 160)
    }
}

struct FakeTz;

impl TimezoneCtl for FakeTz {
    fn list(&self) -> Vec<String> {
        vec!["UTC".into()]
    }
    fn get(&self) -> Result<String, AppError> {
        Ok("UTC".into())
    }
    fn set(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
}

struct World {
    rig: Rig,
    hub: Arc<EventHub>,
    router: Arc<Router>,
    /// What each connection's socket would have been sent.
    owner: (ConnId, Receiver<String>),
    other: (ConnId, Receiver<String>),
}

fn world() -> World {
    let rig = Rig::pending(PASSWORD);
    let hub = Arc::new(EventHub::new());
    let sink: Arc<dyn EventSink> = hub.clone();
    let settings = Arc::new(SettingsHolder::load(Arc::new(MemStore { initial: Settings::default(), ..MemStore::default() })));
    let display = DisplayService::new(Arc::new(FakeBacklight), settings.clone(), sink.clone());
    let timezone = TimezoneService::new(Arc::new(FakeTz), settings, sink.clone());
    let (bt_tx, _bt_rx) = channel::<AgentEvent>();
    let (wifi_tx, _wifi_rx) = channel();
    let router = Arc::new(Router::new(
        Arc::new(BluetoothService::new(Arc::new(UnavailableBluetooth), sink, bt_tx)),
        Arc::new(Wifi::new(wifi_tx)),
        display,
        timezone,
    ));
    router.attach_replies(hub.clone());
    router.attach_users(rig.service.clone());
    let (tx_a, rx_a) = sync_channel::<String>(64);
    let (tx_b, rx_b) = sync_channel::<String>(64);
    let a = hub.register(tx_a, None);
    let b = hub.register(tx_b, None);
    World { rig, hub, router, owner: (a, rx_a), other: (b, rx_b) }
}

impl World {
    /// Sends a raw JSON command as the owner connection and returns the
    /// owner's reply lines.
    fn send(&self, json: &str) -> Vec<String> {
        let cmd: ClientCommand = serde_json::from_str(json).unwrap();
        // The same line the websocket reader writes at debug level.
        log::debug!("client command: {cmd:?}");
        self.router.execute(self.owner.0, cmd);
        self.owner.1.try_iter().collect()
    }

    fn ack(&self, last4: &str) -> Vec<String> {
        self.send(&format!(r#"{{"type":"user_bootstrap_ack","last4":"{last4}"}}"#))
    }

    fn reveal(&self) -> Vec<String> {
        self.send(r#"{"type":"user_bootstrap_reveal"}"#)
    }
}

/// Everything a failure path may say to a client must be free of the secrets.
fn assert_no_leak(lines: &[String], what: &str) {
    for line in lines {
        for needle in [PASSWORD, LAST4, WRONG] {
            assert!(!line.contains(needle), "{what}: {needle} in reply {line}");
        }
    }
}

fn assert_log_clean(what: &str) {
    let log = captured();
    for needle in [PASSWORD, LAST4, WRONG] {
        assert!(!log.contains(needle), "{what}: {needle:?} reached the log:\n{log}");
    }
}

#[test]
fn reveal_ack_and_every_failure_path_keep_the_secrets_out_of_logs_and_errors() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let w = world();

    // No press yet: refused.
    let refused = w.reveal();
    assert!(refused[0].contains(r#""code":"press_required""#), "{refused:?}");
    assert_no_leak(&refused, "press_required");

    // Status never carries the secret.
    let status = w.send(r#"{"type":"user_bootstrap_get"}"#);
    assert!(status[0].contains(r#""pending":true"#), "{status:?}");
    assert_no_leak(&status, "status");

    // Reveal after a physical press: the owner gets it, nobody else does.
    w.rig.presses.record();
    let revealed = w.reveal();
    assert_eq!(revealed.len(), 1, "{revealed:?}");
    assert!(revealed[0].contains("user_bootstrap_secret") && revealed[0].contains(PASSWORD), "{revealed:?}");
    assert!(w.other.1.try_recv().is_err(), "the other connection must receive nothing");

    // Wrong proofs, a rate limited attempt (with the right value), then a
    // bad_ack message check.
    for _ in 0..4 {
        let wrong = w.ack(WRONG);
        assert!(wrong[0].contains(r#""code":"bad_ack""#), "{wrong:?}");
        assert_no_leak(&wrong, "bad_ack");
    }
    let limited = w.ack(LAST4);
    assert!(limited[0].contains(r#""code":"rate_limited""#), "{limited:?}");
    assert_no_leak(&limited, "rate_limited");
    w.rig.timer.advance(Duration::from_secs(5));

    // A reveal whose requester is already gone is dropped, not broadcast.
    w.rig.presses.record();
    w.hub.unregister(w.owner.0);
    let gone = w.reveal();
    assert!(gone.is_empty());
    assert!(w.other.1.try_recv().is_err(), "a reply for a closed connection must not be rebroadcast");
    let (tx, rx) = sync_channel::<String>(64);
    let owner = (w.hub.register(tx, None), rx);

    // Right proof acknowledges and scrubs.
    let cmd: ClientCommand = serde_json::from_str(&format!(r#"{{"type":"user_bootstrap_ack","last4":"{LAST4}"}}"#)).unwrap();
    w.router.execute(owner.0, cmd);
    let acked: Vec<String> = owner.1.try_iter().collect();
    assert!(acked[0].contains(r#""acknowledged":true"#), "{acked:?}");
    assert_no_leak(&acked, "ack");

    // After the ack there is nothing to reveal.
    w.rig.presses.record();
    let cmd: ClientCommand = serde_json::from_str(r#"{"type":"user_bootstrap_reveal"}"#).unwrap();
    w.router.execute(owner.0, cmd);
    let late: Vec<String> = owner.1.try_iter().collect();
    assert!(late[0].contains(r#""code":"not_pending""#), "{late:?}");
    assert_no_leak(&late, "after ack");
    assert!(w.rig.secrets.held.lock().unwrap().is_none());

    assert_log_clean("reveal, ack and wrong ack");
    let log = captured();
    assert!(log.contains("revealed to connection"), "the audit line is still written: {log}");
}

#[test]
fn expiry_storage_and_write_failures_do_not_leak_either() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();

    // Failed state write and failed scrub during an ack.
    let w = world();
    *w.rig.state.fail_save.lock().unwrap() = true;
    let failed = w.ack(LAST4);
    assert!(failed[0].contains(r#""code":"internal""#), "{failed:?}");
    assert_no_leak(&failed, "state write failure");
    *w.rig.state.fail_save.lock().unwrap() = false;
    *w.rig.secrets.fail_scrub.lock().unwrap() = true;
    let failed = w.ack(LAST4);
    assert!(failed[0].contains(r#""code":"internal""#), "{failed:?}");
    assert_no_leak(&failed, "scrub failure");
    *w.rig.secrets.fail_scrub.lock().unwrap() = false;
    w.rig.service.tick();
    assert!(w.rig.secrets.held.lock().unwrap().is_none());

    // Expiry by the timer, then every request against it.
    let w = world();
    w.rig.set_now(900);
    w.rig.service.tick();
    w.rig.presses.record();
    for json in [r#"{"type":"user_bootstrap_reveal"}"#.to_string(), format!(r#"{{"type":"user_bootstrap_ack","last4":"{LAST4}"}}"#)] {
        let lines = w.send(&json);
        assert!(lines[0].contains(r#""code":"expired""#), "{lines:?}");
        assert_no_leak(&lines, "expired");
    }

    // Degraded storage.
    let w = world();
    *w.rig.files.available.lock().unwrap() = false;
    w.rig.presses.record();
    let lines = w.reveal();
    assert!(lines[0].contains(r#""code":"storage_unavailable""#), "{lines:?}");
    assert_no_leak(&lines, "degraded");

    assert_log_clean("expiry, storage and write failures");
}

#[test]
fn debug_output_of_commands_events_and_the_secret_hides_the_values() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let w = world();
    w.rig.presses.record();
    let revealed = w.rig.service.bootstrap_reveal(ConnId(1));
    let rendered = format!("{revealed:?} {revealed:#?}");
    assert!(!rendered.contains(PASSWORD), "{rendered}");
    assert!(rendered.contains("<redacted>"));
    let ack: ClientCommand = serde_json::from_str(&format!(r#"{{"type":"user_bootstrap_ack","last4":"{LAST4}"}}"#)).unwrap();
    let rendered = format!("{ack:?} {ack:#?}");
    assert!(!rendered.contains(LAST4), "{rendered}");
    // Every reply is a user_* event, whose logging is type only.
    for e in [w.rig.service.bootstrap_get(), revealed] {
        assert!(matches!(e, BridgeEvent::UserBootstrap { .. } | BridgeEvent::UserBootstrapSecret { .. }));
        w.hub.emit_to(w.owner.0, e);
    }
    assert_log_clean("debug and targeted logging");
}
