//! terminal-agent: on-device PTY-to-WebSocket relay for the terminal feature.
//!
//! Spawns one PTY-backed child per configured session (a local shell) and relays PTY I/O to WebSocket clients on
//! ws://127.0.0.1:8766 by default (override with `[server] bind` or
//! TERMINAL_AGENT_BIND, see net.rs). The wire protocol is documented in
//! protocol.rs and docs/terminal.md. Sync and threaded on purpose: plain
//! `tungstenite`, one OS thread per client, no async runtime.
//!
//! Module map: `utils` (config, logging, types), `protocol` (typed messages),
//! `pty` (all unsafe libc), `session` (child lifecycle and reader),
//! `respawn_policy` (pure), `hub` (bounded client fanout), `server`
//! (accept loop and dispatch), `clock` (isolated clock sync), `net` (bind
//! and Origin policy), `utf8` (chunk-safe decoding).

pub mod clock;
pub mod hub;
pub mod net;
pub mod protocol;
pub mod pty;
pub mod respawn_policy;
pub mod server;
pub mod session;
pub mod utf8;
pub mod utils;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use utils::config::Config;
pub use utils::types::StartupError;

/// Builds sessions, starts reader and clock threads, binds the listener and
/// runs the accept loop. Returns only on a startup error.
pub fn run(config: Config) -> Result<(), StartupError> {
    let env_bind = std::env::var(net::BIND_ENV).ok();
    let bind_addr = net::resolve_bind(env_bind.as_deref(), &config.server.bind)
        .map_err(|message| StartupError { exit_code: utils::config::EXIT_CONFIG, message })?;
    if !bind_addr.ip().is_loopback() {
        log::warn!(
            "listening on non-loopback address {bind_addr}. This is an UNAUTHENTICATED SHELL \
             reachable from the network. Use only on a trusted link."
        );
    }

    let sessions: Vec<Arc<session::Session>> = config
        .sessions
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let name = c.name.clone().unwrap_or_else(|| format!("session-{i}"));
            match session::Session::new(i, name, c.command.clone(), &config.pty, &config.respawn) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::error!("{e} (session skipped, others unaffected)");
                    None
                }
            }
        })
        .collect();
    if sessions.is_empty() {
        log::warn!("zero sessions spawned successfully, WebSocket server will start with nothing to serve");
    }
    let sessions = Arc::new(sessions);
    let hub = hub::ClientHub::new(config.server.client_queue_capacity);

    for s in sessions.iter() {
        session::spawn_reader(s, &hub, s.generation());
    }

    let clock = if config.clock.enabled {
        let clock: utils::types::SharedClock = Arc::new(Mutex::new(clock::ClockState::new()));
        let (hub, shared, cfg) = (hub.clone(), clock.clone(), config.clock.clone());
        thread::spawn(move || clock::run(hub, shared, cfg));
        Some(clock)
    } else {
        log::info!("clock sync disabled by config");
        None
    };

    let listener = TcpListener::bind(bind_addr)
        .map_err(|e| StartupError { exit_code: 1, message: format!("bind {bind_addr} failed: {e}") })?;
    log::info!("WebSocket server listening on ws://{bind_addr}");
    log::info!("sessions: {:?}", sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());

    server::serve(listener, server::ServerCtx { sessions, hub, clock, config: config.server });
    Ok(())
}
