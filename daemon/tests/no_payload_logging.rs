//! Privacy rule: PTY output and user keystrokes are never logged, at any
//! level. This test installs a capturing logger at `trace`, pushes a unique
//! secret through a real PTY (as input and as output) and asserts that no
//! log record contains it. It lives in its own test binary because the
//! global logger can be installed only once per process.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use terminal_agent::hub::ClientHub;
use terminal_agent::protocol::{decode_client, ServerMsg};
use terminal_agent::server::dispatch;
use terminal_agent::session::{spawn_reader, Session};
use terminal_agent::utils::config::{PtyConfig, RespawnConfig};

struct Capture;
static RECORDS: OnceLock<Mutex<Vec<(Level, String)>>> = OnceLock::new();

impl Log for Capture {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }
    fn log(&self, r: &Record) {
        RECORDS.get().unwrap().lock().unwrap().push((r.level(), format!("{}", r.args())));
    }
    fn flush(&self) {}
}

#[test]
fn secrets_never_reach_the_log() {
    RECORDS.set(Mutex::new(Vec::new())).unwrap();
    log::set_logger(&Capture).unwrap();
    log::set_max_level(LevelFilter::Trace);

    const IN_SECRET: &str = "KEYSTROKE-SECRET-4711";
    const OUT_SECRET: &str = "OUTPUT-SECRET-0815";
    // The quotes split the word in the command line (which is config and may
    // be logged) but the shell prints it joined, so only PTY output carries it.
    let script = "echo OUTPUT-SE\"\"CRET-0815; exec cat".to_string();
    let s = Session::new(
        0,
        "t".into(),
        vec!["sh".into(), "-c".into(), script],
        &PtyConfig::default(),
        &RespawnConfig::default(),
    )
    .unwrap();
    let hub = ClientHub::new(256);
    let rx = hub.register();
    spawn_reader(&s, &hub, s.generation());
    let sessions = vec![s.clone()];
    let send = |raw: &str| dispatch(&decode_client(raw).unwrap(), &sessions, &hub);
    send(&format!(r#"{{"type":"input","session":0,"data":"{IN_SECRET}\n"}}"#));

    // Wait until both secrets have come back out through the output path.
    let mut acc = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !(acc.contains(OUT_SECRET) && acc.matches(IN_SECRET).count() >= 2) {
        if let Ok(m) = rx.recv_timeout(Duration::from_millis(100)) {
            if let Ok(ServerMsg::Output { data, .. }) = serde_json::from_str(&m) {
                acc.push_str(&data);
            }
        }
    }
    assert!(acc.contains(OUT_SECRET) && acc.contains(IN_SECRET), "PTY round trip failed: {acc:?}");

    // Exercise the exit path as well, then give logging a moment to settle.
    send(r#"{"type":"control","session":0,"action":"respawn"}"#);
    thread::sleep(Duration::from_millis(300));

    let records = RECORDS.get().unwrap().lock().unwrap();
    assert!(!records.is_empty(), "capture logger saw nothing; test is not exercising logging");
    for (level, msg) in records.iter() {
        assert!(
            !msg.contains(IN_SECRET) && !msg.contains(OUT_SECRET),
            "{level} log leaked payload: {msg}"
        );
    }
}
