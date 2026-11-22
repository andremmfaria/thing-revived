//! terminal-agent: on-device PTY-to-WebSocket relay for the terminal feature.
//!
//! Spawns one PTY-backed child per configured session (a local shell, or
//! `ssh <host>`) and relays PTY I/O to WebSocket clients on
//! ws://127.0.0.1:8766 by default (override with TERMINAL_AGENT_BIND, see
//! net.rs). The wire protocol is documented in protocol.rs and docs/terminal.md.
//! Sync and threaded on purpose: plain `tungstenite`, one OS thread per
//! client, no async runtime.

pub mod hub;
pub mod net;
pub mod protocol;
pub mod utf8;
