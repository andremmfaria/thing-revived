//! WebSocket server: loopback bind (with the `CONTROLLER_BIND` override and its
//! non-loopback warning), the accept loop, and one reader plus one writer
//! thread per client. Commands go to the `Router`; outbound events come from
//! the `EventHub` through a bounded per-client queue.

use crate::app::hub::{EventHub, TargetedSink};
use crate::app::router::Router;
use crate::protocol::{BridgeEvent, ClientCommand};
use crate::utils::config::{Config, ServerConfig};
use log::{debug, error, info, warn};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{accept_hdr, Message};

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

/// Whether the handshake `Origin` header value is allowed.
///
/// Accepted: header absent (non-browser clients), the literal `null`
/// (Chromium sends it for `file://` pages, which is how the kiosk frontend is
/// loaded), and `http://localhost` or `http://127.0.0.1` with an optional
/// numeric port. The host is compared exactly, so `http://localhost.evil.com`
/// is rejected, as are https and every other host. This stops web pages
/// from foreign origins only: any other local process omits or forges the
/// header, so it is defence in depth and not authentication. Same rule as
/// `net::origin_allowed` in the terminal agent.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    let origin = match origin {
        None => return true,
        Some(o) => o.trim().to_ascii_lowercase(),
    };
    if origin == "null" {
        return true;
    }
    let Some(rest) = origin.strip_prefix("http://") else { return false };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if host != "localhost" && host != "127.0.0.1" {
        return false;
    }
    match port {
        None => true,
        Some(p) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// What the reader logs for a command at debug level. `Secret` fields render
/// as `<redacted>`, but an account command also carries names, keys and
/// fingerprints, and a user may have typed a password into a name field, so
/// for the `user_*`, `group_*` and `ssh_key_*` families only the variant name
/// is logged.
pub fn describe_command(cmd: &ClientCommand) -> String {
    use ClientCommand as C;
    let full = format!("{cmd:?}");
    let account = matches!(
        cmd,
        C::UserBootstrapGet {}
            | C::UserUnlock { .. }
            | C::UserLockSession {}
            | C::UserListGet {}
            | C::UserAdd { .. }
            | C::UserRemove { .. }
            | C::UserSetLocked { .. }
            | C::UserSetPassword { .. }
            | C::GroupListGet {}
            | C::UserGroupAdd { .. }
            | C::UserGroupRemove { .. }
            | C::GroupAdd { .. }
            | C::GroupRemove { .. }
            | C::SshKeyListGet { .. }
            | C::SshKeyAdd { .. }
            | C::SshKeyRemove { .. }
    );
    if account {
        full.chars().take_while(|c| c.is_alphanumeric()).collect()
    } else {
        full
    }
}

/// Handshake callback: a 403 before the upgrade for a foreign origin.
/// The large Err type is dictated by tungstenite's callback signature.
#[allow(clippy::result_large_err)]
fn check_origin(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if origin_allowed(origin) {
        Ok(resp)
    } else {
        warn!("rejected websocket handshake, origin {origin:?}");
        let mut err = ErrorResponse::new(Some("forbidden origin".to_string()));
        *err.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
        Err(err)
    }
}

/// What a client connection needs: the hub to register with and the router
/// its commands go to.
struct ClientContext {
    clients: Arc<EventHub>,
    router: Arc<Router>,
    server: ServerConfig,
}

/// One OS thread pair per client connection, same split-read/write-socket
/// pattern the terminal agent's own `handle_client` in daemon/ uses (see that
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
    let mut ws_read = match accept_hdr(stream, check_origin) {
        Ok(s) => s,
        Err(e) => {
            warn!("websocket handshake failed: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    let (tx, rx) = sync_channel::<String>(state.server.client_queue_capacity);
    let conn = state.clients.register(tx, Some(shutdown_stream));
    info!("ui client {} connected", conn.0);

    let ping_interval = Duration::from_secs(state.server.ping_interval_secs);
    thread::spawn(move || loop {
        match rx.recv_timeout(ping_interval) {
            Ok(json) => {
                if ws_write.send(Message::Text(json)).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle ping so dead connections get noticed promptly.
                if ws_write.send(Message::Ping(Vec::new())).is_err() {
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
                        debug!("client command: {}", describe_command(&cmd));
                        state.router.handle(conn, cmd)
                    }
                    Err(e) => {
                        // Fixed text only: serde's message can echo field values
                        // (e.g. a mistyped wifi password), so it is neither logged
                        // nor sent. The error carries only the line/column.
                        warn!("ignoring unrecognized client message (line {}, column {})", e.line(), e.column());
                        state.clients.emit_to(
                            conn,
                            BridgeEvent::SettingsError {
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
    info!("ui client {} disconnected", conn.0);
    // Dropping the slot closes the writer's channel, which ends its thread.
    state.clients.unregister(conn);
    // The first-boot gate must stop counting this connection as a screen.
    state.router.connection_closed(conn);
}

/// Binds (loopback unless deliberately overridden) and serves clients forever.
pub fn serve(cfg: &Config, clients: Arc<EventHub>, router: Arc<Router>) {
    router.attach_replies(clients.clone());
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
    fn account_commands_log_their_variant_only_and_others_log_in_full() {
        let unlock: ClientCommand = serde_json::from_str(r#"{"type":"user_unlock","username":"Typed-Secret-Value-1","password":"x"}"#).unwrap();
        assert_eq!(describe_command(&unlock), "UserUnlock");
        let add: ClientCommand = serde_json::from_str(r#"{"type":"ssh_key_add","name":"ana","key":"ssh-ed25519 AAAA"}"#).unwrap();
        assert_eq!(describe_command(&add), "SshKeyAdd");
        let wifi: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        let shown = describe_command(&wifi);
        assert!(shown.contains("Home") && !shown.contains("hunter2"), "{shown}");
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
    fn origin_policy() {
        for ok in [None, Some("null"), Some("http://localhost"), Some("http://localhost:8080"), Some("http://127.0.0.1:8765"), Some("HTTP://LocalHost:1")] {
            assert!(origin_allowed(ok), "{ok:?}");
        }
        for bad in [
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://localhost:",
            "http://localhost:80a",
            "https://localhost",
            "http://example.com",
            "file://",
            "",
        ] {
            assert!(!origin_allowed(Some(bad)), "{bad:?}");
        }
    }

    #[test]
    fn listener_binds_loopback_by_default() {
        let mut addr = bind_addr(None, default_bind());
        addr.set_port(0);
        let l = TcpListener::bind(addr).unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }

}
