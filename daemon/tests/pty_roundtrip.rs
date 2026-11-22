//! Real-PTY integration tests (plain Linux, no device needed): the parts a
//! fake PTY cannot cover, namely controlling-terminal/SIGWINCH behavior,
//! process-group signals and respawn output.

use std::fs;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use terminal_agent::hub::ClientHub;
use terminal_agent::protocol::{decode_client, ServerMsg};
use terminal_agent::server::dispatch;
use terminal_agent::session::{spawn_reader, Session};
use terminal_agent::utils::config::{PtyConfig, RespawnConfig};
use terminal_agent::utils::types::Msg;

struct Rig {
    session: Arc<Session>,
    sessions: Vec<Arc<Session>>,
    hub: ClientHub,
    rx: Receiver<Msg>,
}

fn rig(script: &str) -> Rig {
    let session = Session::new(
        0,
        "t".into(),
        vec!["sh".into(), "-c".into(), script.into()],
        &PtyConfig::default(),
        &RespawnConfig::default(),
    )
    .unwrap();
    let hub = ClientHub::new(256);
    let rx = hub.register();
    spawn_reader(&session, &hub, session.generation());
    Rig { sessions: vec![session.clone()], session, hub, rx }
}

impl Rig {
    fn send(&self, raw: &str) {
        dispatch(&decode_client(raw).unwrap(), &self.sessions, &self.hub);
    }

    /// Next message matching `pred` within `secs`.
    fn wait(&self, secs: u64, pred: impl Fn(&ServerMsg) -> bool) -> Option<ServerMsg> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(m) = self.rx.recv_timeout(left) {
                let v: ServerMsg = serde_json::from_str(&m).unwrap();
                if pred(&v) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Waits until the accumulated output contains `needle`.
    fn wait_output(&self, secs: u64, needle: &str) -> bool {
        let mut acc = String::new();
        let deadline = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(m) = self.rx.recv_timeout(left) {
                if let Ok(ServerMsg::Output { data, .. }) = serde_json::from_str(&m) {
                    acc.push_str(&data);
                    if acc.contains(needle) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[test]
fn echo_round_trip_with_multibyte_input() {
    let r = rig("exec cat");
    r.send(r#"{"type":"resize","session":0,"cols":80,"rows":24}"#);
    r.send(r#"{"type":"input","session":0,"data":"h\u00e9llo w\u00f6rld\n"}"#);
    assert!(r.wait_output(5, "h\u{e9}llo w\u{f6}rld"));
}

/// The setsid + TIOCSCTTY workaround: resizing the master must reach the
/// child as SIGWINCH, and the child must see the new size.
#[test]
fn resize_delivers_sigwinch() {
    let r = rig("trap 'echo WINCH:$(stty size)' WINCH; echo ready; while :; do sleep 0.05; done");
    assert!(r.wait_output(5, "ready"));
    r.send(r#"{"type":"resize","session":0,"cols":100,"rows":30}"#);
    assert!(r.wait_output(5, "WINCH:30 100"));
}

fn pgrp_members(pgid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for e in fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // Fields after the parenthesized comm: state ppid pgrp ...
        let rest = stat.rsplit(')').next().unwrap_or("").split_whitespace().collect::<Vec<_>>();
        if rest.first() == Some(&"Z") {
            continue;
        }
        if rest.get(2).and_then(|p| p.parse::<u32>().ok()) == Some(pgid) {
            out.push(pid);
        }
    }
    out
}

#[test]
fn interrupt_kills_the_whole_process_group() {
    // A pipeline puts three processes (sh, sleep, cat) in the session's group.
    let r = rig("echo up; sleep 300 | cat");
    assert!(r.wait_output(5, "up"));
    let pgid = r.session.pid().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while pgrp_members(pgid).len() < 3 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(pgrp_members(pgid).len() >= 3, "pipeline never started: {:?}", pgrp_members(pgid));

    r.send(r#"{"type":"control","session":0,"action":"interrupt"}"#);

    let exited = r.wait(5, |m| matches!(m, ServerMsg::Exited { .. })).expect("exited message after SIGINT");
    // Killed by a signal, so no exit code.
    assert!(matches!(exited, ServerMsg::Exited { code: None, auto_respawning: true, .. }), "{exited:?}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pgrp_members(pgid).is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    // The 0 s auto-respawn starts a new child in a new group; the old group is gone.
    assert!(pgrp_members(pgid).is_empty(), "old group survived: {:?}", pgrp_members(pgid));
}

#[test]
fn respawn_gives_fresh_output_from_a_new_child() {
    let r = rig("echo started:$$; exec cat");
    let first = r
        .wait(5, |m| matches!(m, ServerMsg::Output { data, .. } if data.contains("started:")))
        .expect("first output");
    let pid_of = |m: &ServerMsg| match m {
        ServerMsg::Output { data, .. } => data.trim().trim_start_matches("started:").to_string(),
        _ => unreachable!(),
    };
    let first_pid = pid_of(&first);
    assert_eq!(first_pid, r.session.pid().unwrap().to_string());

    r.send(r#"{"type":"control","session":0,"action":"respawn"}"#);
    let second = r
        .wait(5, |m| matches!(m, ServerMsg::Output { data, .. } if data.contains("started:")))
        .expect("output after respawn");
    assert_ne!(pid_of(&second), first_pid);
    assert_eq!(pid_of(&second), r.session.pid().unwrap().to_string());

    // The respawned child still echoes, through exactly one reader: input
    // comes back once, and no `exited` was broadcast for the manual respawn.
    r.send(r#"{"type":"input","session":0,"data":"ping\n"}"#);
    thread::sleep(Duration::from_millis(500));
    let msgs: Vec<ServerMsg> = r.rx.try_iter().map(|m| serde_json::from_str(&m).unwrap()).collect();
    assert!(!msgs.iter().any(|m| matches!(m, ServerMsg::Exited { .. })), "{msgs:?}");
    let joined: String = msgs
        .iter()
        .filter_map(|m| if let ServerMsg::Output { data, .. } = m { Some(data.as_str()) } else { None })
        .collect();
    // A PTY echoes typed input once and `cat` prints it once: "ping\r\nping\r\n".
    assert_eq!(joined.matches("ping").count(), 2, "{joined:?}");
    assert_eq!(r.session.generation(), 2);
}

#[test]
fn double_respawn_leaves_a_single_output_stream() {
    let r = rig("exec cat");
    r.send(r#"{"type":"control","session":0,"action":"respawn"}"#);
    r.send(r#"{"type":"control","session":0,"action":"respawn"}"#);
    assert_eq!(r.session.generation(), 3);
    r.send(r#"{"type":"input","session":0,"data":"x1\n"}"#);
    thread::sleep(Duration::from_millis(700));
    let joined: String = r
        .rx
        .try_iter()
        .filter_map(|m| match serde_json::from_str(&m).unwrap() {
            ServerMsg::Output { data, .. } => Some(data),
            ServerMsg::Exited { .. } => panic!("exited broadcast during manual respawns"),
            _ => None,
        })
        .collect();
    assert_eq!(joined.matches("x1").count(), 2, "{joined:?}");
}
