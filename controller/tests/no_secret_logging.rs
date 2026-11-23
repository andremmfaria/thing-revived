//! End to end: the generated first-boot password never reaches a log line, an
//! error string, a `Debug` rendering, a broadcast, or any connection other
//! than the ready frontend that asked. A capturing `log` backend records every
//! line any module writes at any level, while the real commands, the M-button
//! gate, the user service (over fake ports) and the event hub run together.

use controller_agent::adapters::unavailable_bluetooth::UnavailableBluetooth;
use controller_agent::app::error::AppError;
use controller_agent::app::hub::{ConnId, EventHub};
use controller_agent::app::router::Router;
use controller_agent::domain::bluetooth::AgentEvent;
use controller_agent::domain::brightness::validate;
use controller_agent::domain::settings::Settings;
use controller_agent::ports::{Backlight, EventSink, FirstBootGate, TimezoneCtl};
use controller_agent::protocol::{BridgeEvent, ClientCommand};
use controller_agent::services::bluetooth::BluetoothService;
use controller_agent::services::display::DisplayService;
use controller_agent::services::settings::testing::MemStore;
use controller_agent::services::settings::SettingsHolder;
use controller_agent::services::timezone::TimezoneService;
use controller_agent::services::users::testing::Rig;
use controller_agent::domain::accounts::state::AccountState;
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
    gate: Arc<dyn FirstBootGate>,
    router: Arc<Router>,
    /// What each connection's socket would have been sent.
    owner: (ConnId, Receiver<String>),
    other: (ConnId, Receiver<String>),
}

fn world() -> World {
    let hub = Arc::new(EventHub::new());
    let rig = Rig::with_sinks("!", None, hub.clone(), hub.clone());
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
    let gate = rig.service.gate();
    World { rig, gate, router, owner: (a, rx_a), other: (b, rx_b) }
}

impl World {
    /// The owner connection sends a raw JSON command; returns its reply lines.
    fn send(&self, json: &str) -> Vec<String> {
        let cmd: ClientCommand = serde_json::from_str(json).unwrap();
        // The same line the websocket reader writes at debug level.
        log::debug!("client command: {cmd:?}");
        self.router.execute(self.owner.0, cmd);
        self.owner.1.try_iter().collect()
    }

    /// One physical M press on the down edge, with the worker joined.
    fn press(&self) {
        assert!(self.gate.active(), "the gate must be armed");
        if let Some(h) = self.rig.service.press() {
            h.join().unwrap();
        }
    }

    fn password(&self) -> String {
        self.rig.accounts.last_password.lock().unwrap().clone().expect("a password was set")
    }
}

fn assert_clean(lines: &[String], needle: &str, what: &str) {
    for line in lines {
        assert!(!line.contains(needle), "{what}: the password is in {line}");
    }
}

fn assert_log_clean(needle: &str, what: &str) {
    let log = captured();
    assert!(!log.contains(needle), "{what}: the password reached the log:\n{log}");
}

#[test]
fn the_password_reaches_the_ready_frontend_only_and_never_a_log_or_a_broadcast() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let w = world();

    // Asking before generation: phase only.
    let status = w.send(r#"{"type":"user_bootstrap_get"}"#);
    assert_eq!(status.len(), 1, "{status:?}");
    assert!(status[0].contains(r#""phase":"prompt""#), "{status:?}");

    // The physical press generates; the owner is ready, the other connection
    // never asked, so it only sees the broadcast phase events.
    w.press();
    let pw = w.password();
    let owner: Vec<String> = w.owner.1.try_iter().collect();
    let other: Vec<String> = w.other.1.try_iter().collect();
    assert!(owner.iter().any(|l| l.contains("user_bootstrap_secret") && l.contains(&pw)), "{owner:?}");
    assert!(owner.iter().any(|l| l.contains(r#""phase":"showing""#)), "{owner:?}");
    assert!(!other.is_empty() && other.iter().all(|l| l.contains(r#""type":"user_bootstrap""#)), "{other:?}");
    assert_clean(&other, &pw, "the connection that never asked");

    // A reconnect gets the same password back, only to the asker.
    let again = w.send(r#"{"type":"user_bootstrap_get"}"#);
    assert!(again.iter().any(|l| l.contains(&pw)), "{again:?}");
    assert!(w.other.1.try_recv().is_err());

    // The second press acknowledges: nobody can ask for it afterwards.
    w.rig.timer.advance(Duration::from_secs(2));
    w.press();
    let done = w.send(r#"{"type":"user_bootstrap_get"}"#);
    assert!(done.iter().all(|l| l.contains(r#""phase":"done""#)), "{done:?}");
    assert_clean(&done, &pw, "after the ack");
    let broadcasts: Vec<String> = w.other.1.try_iter().collect();
    assert_clean(&broadcasts, &pw, "broadcast phase events");

    assert_log_clean(&pw, "generate, show and ack");
    assert!(captured().contains("user_bootstrap_secret (payload not logged)"), "the type is still logged");
}

#[test]
fn failure_paths_say_nothing_about_the_password() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();

    // Ack write failure.
    let w = world();
    w.send(r#"{"type":"user_bootstrap_get"}"#);
    w.press();
    let pw = w.password();
    w.owner.1.try_iter().for_each(drop);
    *w.rig.state.fail_save.lock().unwrap() = true;
    w.rig.timer.advance(Duration::from_secs(2));
    w.press();
    let failed: Vec<String> = w.owner.1.try_iter().collect();
    assert!(failed.iter().any(|l| l.contains(r#""code":"ack_failed""#)), "{failed:?}");
    assert_clean(&failed, &pw, "ack_failed");

    // Generation failure (chpasswd): no password ever existed to leak, but
    // the error text is fixed and the log is checked against the plaintext
    // the generator produced before the failure.
    let w = world();
    w.send(r#"{"type":"user_bootstrap_get"}"#);
    *w.rig.accounts.fail_set.lock().unwrap() = true;
    w.press();
    let failed: Vec<String> = w.owner.1.try_iter().collect();
    assert!(failed.iter().any(|l| l.contains(r#""code":"generate_failed""#)), "{failed:?}");
    *w.rig.accounts.fail_set.lock().unwrap() = false;
    w.press();
    let pw2 = w.password();
    assert_log_clean(&pw, "ack failure");
    assert_log_clean(&pw2, "generation failure then success");

    // Degraded storage.
    let w = world();
    *w.rig.files.available.lock().unwrap() = false;
    let lines = w.send(r#"{"type":"user_bootstrap_get"}"#);
    assert!(lines[0].contains(r#""storage_ok":false"#), "{lines:?}");
}

#[test]
fn debug_output_of_commands_events_and_the_secret_hides_the_value() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let w = world();
    w.send(r#"{"type":"user_bootstrap_get"}"#);
    w.press();
    let pw = w.password();
    let secret_line = w.owner.1.try_iter().find(|l| l.contains("user_bootstrap_secret")).expect("the owner got the secret");
    assert!(secret_line.contains(&pw));
    let event = BridgeEvent::UserBootstrapSecret { username: "kiosk".into(), password: serde_json::from_str(&format!("\"{pw}\"")).unwrap() };
    let rendered = format!("{event:?} {event:#?}");
    assert!(!rendered.contains(&pw), "{rendered}");
    assert!(rendered.contains("<redacted>"));
    // A user_unlock password in a command is redacted in Debug too.
    let unlock: ClientCommand = serde_json::from_str(r#"{"type":"user_unlock","username":"ana","password":"hunter2hunter2"}"#).unwrap();
    assert!(!format!("{unlock:?}").contains("hunter2"));
    // The placeholder reply carries no part of the command.
    let reply = w.send(r#"{"type":"user_unlock","username":"ana","password":"hunter2hunter2"}"#);
    assert!(reply[0].contains("not_implemented"), "{reply:?}");
    assert_clean(&reply, "hunter2", "placeholder reply");
    assert_log_clean("hunter2", "command debug line");
    assert_log_clean(&pw, "debug and targeted logging");
}

#[test]
fn a_restart_while_showing_leaves_no_plaintext_anywhere_but_memory() {
    let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let w = world();
    w.send(r#"{"type":"user_bootstrap_get"}"#);
    w.press();
    let pw = w.password();
    // The only persisted artefacts: the state file (no password field) and
    // the hash in the fake shadow.
    let state: AccountState = w.rig.state.held.lock().unwrap().clone().unwrap();
    assert!(!format!("{state:?}").contains(&pw));
    assert!(!w.rig.files.hash().contains(&pw));
    assert_log_clean(&pw, "state and hash");
}
