//! WebSocket server: loopback bind (with the `CONTROLLER_BIND` override and its
//! non-loopback warning), the accept loop, and one reader plus one writer
//! thread per client. Commands go to the `Router`; outbound events come from
//! the `EventHub` through a bounded per-client queue.

use crate::app::hub::EventHub;
use crate::app::router::Router;
use crate::ports::EventSink;
use crate::protocol::{BridgeEvent, ClientCommand};
use crate::utils::config::{Config, ServerConfig};
use log::{debug, error, info, warn};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{accept, Message};

/// Compiled-in WebSocket bind (also `[server] bind`'s default). The only
/// legitimate client is the Chromium kiosk on the device itself, and this
/// socket can change Wi-Fi, Bluetooth and system settings with no
/// authentication, so it must not be reachable from the network. From a dev
/// host use `ssh -L 8765:127.0.0.1:8765 <device>` instead of a wide bind.
pub const DEFAULT_BIND: &str = "127.0.0.1:8765";

/// Env var a developer can set to deliberately override `[server] bind`
/// (e.g. `CONTROLLER_BIND=0.0.0.0:8765`). A non-loopback value is honoured
/// but logged loudly in `run`. An unparsable value falls back to the
/// loopback default rather than to anything wider.
pub const BIND_ENV: &str = "CONTROLLER_BIND";

/// Resolves the listen address from the raw env value (`None` = unset) and
/// the configured `[server] bind`. The env var has priority over the file.
pub fn bind_addr(env_value: Option<&str>, configured: SocketAddr) -> SocketAddr {
    match env_value {
        None => configured,
        Some(raw) => raw.trim().parse().unwrap_or_else(|e| {
            warn!("{BIND_ENV}={raw:?} is not a valid host:port ({e}); using {DEFAULT_BIND}");
            DEFAULT_BIND.parse().expect("DEFAULT_BIND is a valid socket address")
        }),
    }
}

pub fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

// Per-client outbound queue depth is `[server] client_queue_capacity`. A
// healthy kiosk drains it instantly; a client that falls that far behind
// (stalled socket) is disconnected instead of letting its queue grow without
// bound on a 512 MB device.

/// What a client connection needs: the hub to register with and the router
/// its commands go to.
struct ClientContext {
    clients: Arc<EventHub>,
    router: Arc<Router>,
    server: ServerConfig,
}

/// One OS thread pair per client connection, same split-read/write-socket
/// pattern terminal/agent's own `handle_client` already uses (see that
/// file's doc comment for the full rationale): a `try_clone()`'d second
/// `TcpStream` backs an independent writer `WebSocket`, so broadcasts
/// (buttons/knob events, BT/brightness/timezone replies) never block on,
/// or get blocked by, this client's own inbound command reads - needed
/// here specifically because `ClientCommand::BtPair` can take a while
/// (Device1.Pair() blocking on its own agent round-trip) without ever
/// touching this function directly (it's spawned onto its own thread by
/// `handle_client_command`), but a slow/idle client must still never
/// stall buttons/knob delivery to every OTHER connected client either.
fn handle_client(stream: TcpStream, state: Arc<ClientContext>) {
    let (write_stream, shutdown_stream) = match (stream.try_clone(), stream.try_clone()) {
        (Ok(w), Ok(c)) => (w, c),
        (Err(e), _) | (_, Err(e)) => {
            error!("failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_read = match accept(stream) {
        Ok(s) => s,
        Err(e) => {
            warn!("websocket handshake failed: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    let (tx, rx) = sync_channel::<String>(state.server.client_queue_capacity);
    state.clients.register(tx, Some(shutdown_stream));
    info!("ui client connected");

    let ping_interval = Duration::from_secs(state.server.ping_interval_secs);
    thread::spawn(move || loop {
        match rx.recv_timeout(ping_interval) {
            Ok(json) => {
                if ws_write.send(Message::Text(json.into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle ping so dead connections get noticed promptly.
                if ws_write.send(Message::Ping(Vec::new().into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    });

    loop {
        match ws_read.read() {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<ClientCommand>(&text) {
                    Ok(cmd) => {
                        // `Secret` renders as <redacted>, so this is safe to log.
                        debug!("client command: {cmd:?}");
                        state.router.handle(cmd)
                    }
                    Err(e) => {
                        // Fixed text only: serde's message can echo field values
                        // (e.g. a mistyped wifi password), so it is neither logged
                        // nor sent. The error carries only the line/column.
                        warn!("ignoring unrecognized client message (line {}, column {})", e.line(), e.column());
                        state.clients.emit(BridgeEvent::SettingsError {
                                context: "client_message".into(),
                                message: "unrecognized or malformed message".into(),
                            },
                        );
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    info!("ui client disconnected");
    // The writer thread's channel sender still lives in `state.clients`;
    // `broadcast` prunes it on the next send once the writer has exited.
}

/// Binds (loopback unless deliberately overridden) and serves clients forever.
pub fn serve(cfg: &Config, clients: Arc<EventHub>, router: Arc<Router>) {
    let state = Arc::new(ClientContext { clients, router, server: cfg.server.clone() });
    let addr = bind_addr(std::env::var(BIND_ENV).ok().as_deref(), cfg.bind_addr());
    if !is_loopback(&addr) {
        warn!(
            "WARNING: the configured bind ({BIND_ENV} or server.bind) is {addr}, which is NOT loopback. The \
             WebSocket API is unauthenticated (Wi-Fi credentials, Bluetooth, settings) and will be reachable \
             from the network. Development only."
        );
    }
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));
    info!("WebSocket server listening on ws://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || handle_client(stream, state));
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_bind() -> SocketAddr {
        Config::default().bind_addr()
    }

    #[test]
    fn default_bind_is_loopback() {
        let addr = bind_addr(None, default_bind());
        assert!(is_loopback(&addr), "{addr}");
        assert_eq!(addr.port(), 8765);
        assert!(is_loopback(&DEFAULT_BIND.parse().unwrap()));
    }

    #[test]
    fn bind_env_override_is_honoured_and_flagged_when_wide() {
        let wide = bind_addr(Some("0.0.0.0:9000"), default_bind());
        assert_eq!(wide.port(), 9000);
        assert!(!is_loopback(&wide));
        assert!(is_loopback(&bind_addr(Some("[::1]:8765"), default_bind())));
    }

    #[test]
    fn invalid_bind_env_falls_back_to_loopback_default() {
        assert_eq!(bind_addr(Some("not an address"), default_bind()), default_bind());
        assert!(is_loopback(&bind_addr(Some(""), default_bind())));
    }

    #[test]
    fn listener_binds_loopback_by_default() {
        let mut addr = bind_addr(None, default_bind());
        addr.set_port(0);
        let l = TcpListener::bind(addr).unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }

}
