//! Golden fixtures: protocol/terminal/*.json hold the exact wire bytes.

use std::fs;
use terminal_agent::protocol::{decode_client, encode, ClientMsg, ServerMsg, SessionInfo};
use terminal_agent::utils::types::{ClockStatus, ControlAction};

fn fixture(name: &str) -> String {
    let path = format!("{}/../protocol/terminal/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    raw.trim_end_matches('\n').to_string()
}

#[test]
fn client_fixtures_decode_and_reencode_byte_stable() {
    let cases = [
        ("client_input", ClientMsg::Input { session: 0, data: "h\u{e9}llo\r".into() }),
        ("client_resize", ClientMsg::Resize { session: 1, cols: 80, rows: 24 }),
        ("client_control_interrupt", ClientMsg::Control { session: 0, action: ControlAction::Interrupt }),
        ("client_control_clear", ClientMsg::Control { session: 0, action: ControlAction::Clear }),
        ("client_control_respawn", ClientMsg::Control { session: 3, action: ControlAction::Respawn }),
    ];
    for (name, expected) in cases {
        let bytes = fixture(name);
        assert_eq!(decode_client(&bytes), Some(expected.clone()), "{name}: decode");
        assert_eq!(encode(&expected), bytes, "{name}: re-encode");
    }
}

#[test]
fn server_fixtures_match_emitted_bytes() {
    let cases = [
        (
            "server_session_list",
            ServerMsg::SessionList {
                sessions: vec![
                    SessionInfo { index: 0, name: "shell-1".into() },
                    SessionInfo { index: 2, name: "shell-3".into() },
                ],
            },
        ),
        ("server_output", ServerMsg::Output { session: 2, data: "h\u{e9}llo\r\n\u{1b}[0m".into() }),
        (
            "server_exited_auto_respawning",
            ServerMsg::Exited { session: 0, code: Some(3), auto_respawning: true, needs_manual_respawn: false },
        ),
        (
            "server_exited_needs_manual_respawn",
            ServerMsg::Exited { session: 1, code: None, auto_respawning: false, needs_manual_respawn: true },
        ),
        ("server_clock_sync_ok", ServerMsg::ClockSync { offset_ms: -42, status: ClockStatus::Ok, age_s: None }),
        ("server_clock_sync_stale", ServerMsg::ClockSync { offset_ms: 1500, status: ClockStatus::Stale, age_s: Some(120) }),
        ("server_clock_sync_unsynced", ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Unsynced, age_s: Some(0) }),
    ];
    for (name, msg) in cases {
        let bytes = fixture(name);
        assert_eq!(encode(&msg), bytes, "{name}: encode");
        let back: ServerMsg = serde_json::from_str(&bytes).unwrap();
        assert_eq!(back, msg, "{name}: decode");
    }
}

#[test]
fn every_fixture_file_is_covered() {
    let dir = format!("{}/../protocol/terminal", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for e in fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".json") {
            n += 1;
        }
    }
    assert_eq!(n, 5 + 7, "fixture added without a golden case");
}
