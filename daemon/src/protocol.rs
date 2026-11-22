//! Typed wire protocol for the terminal agent (ws://127.0.0.1:8766).
//!
//! One JSON object per text frame, tagged by a snake_case `type`. Canonical
//! examples of every variant live in `protocol/terminal/*.json` and are
//! asserted byte for byte by `tests/protocol_golden.rs`.
//!
//! agent -> frontend (`ServerMsg`):
//!   {"type":"session_list","sessions":[{"index":0,"name":"..."}, ...]}
//!   {"type":"output","session":N,"data":"<decoded pty stdout chunk>"}
//!   {"type":"exited","session":N,"code":Option<i32>,
//!    "auto_respawning":bool,"needs_manual_respawn":bool}
//!   {"type":"clock_sync","offset_ms":<i64>,"status":"ok"|"stale"|"unsynced",
//!    "age_s":<u64, present when status != "ok">}
//! frontend -> agent (`ClientMsg`):
//!   {"type":"input","session":N,"data":"<raw keystrokes>"}
//!   {"type":"resize","session":N,"cols":C,"rows":R}
//!   {"type":"control","session":N,"action":"interrupt"|"clear"|"respawn"}
//!
//! Unknown or malformed client frames decode to `None` and are ignored, as
//! they always were. There is no error message type.
//!
//! Key order: the agent has always emitted keys alphabetically (a
//! `serde_json::Value` map is key-sorted), so `encode` goes through `Value`
//! to keep the bytes identical to the pre-typed implementation. A derived
//! internally tagged `Serialize` would emit `type` first and change the bytes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Interrupt,
    Clear,
    Respawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Input { session: u64, data: String },
    // cols/rows stay u64 and are narrowed with `as u16` by the dispatcher,
    // exactly like the untyped implementation did.
    Resize { session: u64, cols: u64, rows: u64 },
    Control { session: u64, action: ControlAction },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClockStatus {
    Unsynced,
    Ok,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub index: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    SessionList { sessions: Vec<SessionInfo> },
    Output { session: usize, data: String },
    Exited { session: usize, code: Option<i32>, auto_respawning: bool, needs_manual_respawn: bool },
    ClockSync {
        offset_ms: i64,
        status: ClockStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        age_s: Option<u64>,
    },
}

/// Parses one client text frame; `None` for anything that is not a valid
/// `ClientMsg` (unknown type, missing field, bad action).
pub fn decode_client(text: &str) -> Option<ClientMsg> {
    serde_json::from_str(text).ok()
}

/// Serializes a message to its wire bytes (compact, keys sorted).
pub fn encode<T: Serialize>(msg: &T) -> String {
    // The message types contain only strings, numbers, bools and options, so
    // conversion to Value cannot fail.
    serde_json::to_value(msg).map(|v| v.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_ignores_unknown_and_malformed() {
        for bad in [
            r#"{"type":"bogus","session":0}"#,
            r#"{"type":"input"}"#,
            r#"{"type":"input","session":0}"#,
            r#"{"type":"control","session":0,"action":"nope"}"#,
            r#"{"type":"resize","session":0,"cols":"80","rows":24}"#,
            "not json",
            "[]",
        ] {
            assert_eq!(decode_client(bad), None, "{bad}");
        }
    }

    #[test]
    fn decode_tolerates_extra_fields() {
        let m = decode_client(r#"{"type":"input","session":1,"data":"x","extra":true}"#);
        assert_eq!(m, Some(ClientMsg::Input { session: 1, data: "x".into() }));
    }

    #[test]
    fn age_s_omitted_when_none() {
        let m = ServerMsg::ClockSync { offset_ms: 1, status: ClockStatus::Ok, age_s: None };
        assert_eq!(encode(&m), r#"{"offset_ms":1,"status":"ok","type":"clock_sync"}"#);
    }
}
