//! WebSocket server: accept loop, handshake (Origin policy and timeout),
//! per-client reader and writer threads, and dispatch of typed client
//! messages onto sessions.

use crate::clock::clock_sync_message;
use crate::hub::ClientHub;
use crate::net;
use crate::protocol::{decode_client, ClientMsg, ServerMsg, SessionInfo};
use crate::session::{spawn_reader, Session};
use crate::utils::config::ServerConfig;
use crate::utils::types::{ControlAction, SharedClock};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::Message;

/// Everything a client thread needs; cheap to clone.
#[derive(Clone)]
pub struct ServerCtx {
    pub sessions: Arc<Vec<Arc<Session>>>,
    pub hub: ClientHub,
    /// `None` when clock sync is disabled: no `clock_sync` is ever sent.
    pub clock: Option<SharedClock>,
    pub config: ServerConfig,
}

/// Handshake callback: rejects browser pages from foreign origins (see
/// net::origin_allowed for the accepted set) with a 403 before upgrading.
/// The large Err type is dictated by tungstenite's callback signature.
#[allow(clippy::result_large_err)]
pub fn check_origin(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if net::origin_allowed(origin) {
        Ok(resp)
    } else {
        log::warn!("rejected websocket handshake, origin {origin:?}");
        let mut err = ErrorResponse::new(Some("forbidden origin".to_string()));
        *err.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
        Err(err)
    }
}

pub fn session_list_message(sessions: &[Arc<Session>]) -> ServerMsg {
    ServerMsg::SessionList {
        sessions: sessions.iter().map(|s| SessionInfo { index: s.index, name: s.name.clone() }).collect(),
    }
}

/// Routes one decoded client message to its session.
pub fn dispatch(msg: &ClientMsg, sessions: &[Arc<Session>], hub: &ClientHub) {
    let idx = match msg {
        ClientMsg::Input { session, .. }
        | ClientMsg::Resize { session, .. }
        | ClientMsg::Control { session, .. } => *session as usize,
    };
    // Look up by each session's own `index` field, not raw vector
    // position: sessions that failed to spawn are absent from this slice
    // entirely, so vector position and the configured `index` can diverge
    // once any session is dropped.
    let session = match sessions.iter().find(|s| s.index == idx) {
        Some(s) => s,
        None => return,
    };
    match msg {
        ClientMsg::Input { data, .. } => session.write(data),
        ClientMsg::Resize { cols, rows, .. } => session.resize(*cols as u16, *rows as u16),
        ClientMsg::Control { action, .. } => match action {
            ControlAction::Interrupt => session.interrupt(),
            ControlAction::Clear => session.clear(),
            ControlAction::Respawn => {
                // Manual respawn (user-triggered) deliberately does NOT touch
                // the respawn policy - repeated manual respawns must never trip
                // the auto-backoff limiter meant for crash-looping commands.
                // A fresh child needs a fresh reader thread, or its output
                // would vanish (confirmed on real hardware before the reader
                // was spawned here). The old reader goes stale via the
                // generation bump inside respawn and exits on its own.
                if let Some(g) = session.respawn() {
                    spawn_reader(session, hub, g);
                }
            }
        },
    }
}

/// Accept loop: one OS thread per client connection. Returns only if the
/// listener itself dies; per-connection accept errors are logged.
pub fn serve(listener: TcpListener, ctx: ServerCtx) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let ctx = ctx.clone();
                thread::spawn(move || handle_client(stream, ctx));
            }
            Err(e) => log::warn!("accept error: {e}"),
        }
    }
}

/// Reading and writing a single sync `tungstenite::WebSocket` concurrently
/// needs two independent handles on the same socket: the handshake accept()
/// keeps the original `TcpStream` for the blocking read loop, and a
/// `try_clone()`'d duplicate (sharing the same underlying socket, a distinct
/// fd) backs a second `WebSocket` used only for writing broadcast messages
/// from a dedicated writer thread. This keeps PTY output from ever blocking
/// on, or being blocked by, reads.
pub fn handle_client(stream: TcpStream, ctx: ServerCtx) {
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("failed to clone client socket: {e}");
            return;
        }
    };
    // A silent peer must not hold this thread forever: bound the handshake.
    // (Socket options are per socket, shared by every dup of it.)
    let handshake_timeout = Duration::from_secs(ctx.config.handshake_timeout_secs);
    let _ = stream.set_read_timeout(Some(handshake_timeout));
    let _ = stream.set_write_timeout(Some(handshake_timeout));
    let mut ws_read = match tungstenite::accept_hdr(stream, check_origin) {
        Ok(ws) => ws,
        Err(e) => {
            log::warn!("websocket handshake failed: {e}");
            return;
        }
    };
    let write_stream_for_timeout = match write_stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    // Handshake done: reads block indefinitely again, and a stalled peer must
    // not wedge the writer thread forever either.
    let _ = ws_read.get_ref().set_read_timeout(None);
    let _ = write_stream_for_timeout
        .set_write_timeout(Some(Duration::from_secs(ctx.config.client_write_timeout_secs)));
    let rx = ctx.hub.register();
    log::info!("frontend client connected");
    ctx.hub.broadcast(&session_list_message(&ctx.sessions));
    // So a freshly opened tab doesn't show "unsynced" for a full sync
    // interval - send the current cached clock state immediately too.
    if let Some(clock) = &ctx.clock {
        ctx.hub.broadcast(&clock_sync_message(clock));
    }

    thread::spawn(move || {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(json) => {
                    if ws_write.send(Message::Text(json.to_string().into())).is_err() {
                        break;
                    }
                    let _ = ws_write.flush();
                }
                Err(RecvTimeoutError::Timeout) => {
                    if ws_write.send(Message::Ping(Vec::new().into())).is_err() {
                        break;
                    }
                    let _ = ws_write.flush();
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // Dropped for being slow, or the socket died: close it so the reader
        // loop below ends too and the client can reconnect cleanly.
        let _ = write_stream_for_timeout.shutdown(Shutdown::Both);
    });

    loop {
        match ws_read.read() {
            Ok(Message::Text(text)) => {
                if let Some(msg) = decode_client(&text) {
                    dispatch(&msg, &ctx.sessions, &ctx.hub);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    log::info!("frontend client disconnected");
    // The writer thread's channel sender still lives in the hub until the
    // next broadcast finds it dead or full; the writer then closes the socket.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::config::{PtyConfig, RespawnConfig};
    use std::io::Write;
    use std::time::Instant;

    fn ctx_with(sessions: Vec<Arc<Session>>, handshake_secs: u64) -> ServerCtx {
        ServerCtx {
            sessions: Arc::new(sessions),
            hub: ClientHub::new(256),
            clock: None,
            config: ServerConfig { handshake_timeout_secs: handshake_secs, ..ServerConfig::default() },
        }
    }

    #[test]
    fn handshake_origin_enforced() {
        let try_origin = |origin: Option<&str>| -> bool {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            let srv = thread::spawn(move || {
                let (s, _) = l.accept().unwrap();
                tungstenite::accept_hdr(s, check_origin).is_ok()
            });
            let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://127.0.0.1:{port}/")).unwrap();
            if let Some(o) = origin {
                req.headers_mut().insert("origin", o.parse().unwrap());
            }
            let c = tungstenite::connect(req).is_ok();
            let s = srv.join().unwrap();
            assert_eq!(c, s);
            s
        };
        assert!(try_origin(None));
        assert!(try_origin(Some("null")));
        assert!(try_origin(Some("http://localhost:8080")));
        assert!(!try_origin(Some("http://evil.example")));
        assert!(!try_origin(Some("http://localhost.evil.example")));
    }

    /// A connection that never speaks must be dropped after the handshake timeout.
    #[test]
    fn silent_connection_is_dropped_after_handshake_timeout() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let ctx = ctx_with(vec![], 1);
        let srv = thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let t = Instant::now();
            handle_client(s, ctx);
            t.elapsed()
        });
        let _silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let elapsed = srv.join().unwrap();
        assert!(elapsed >= Duration::from_millis(900), "returned early: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(5), "never timed out: {elapsed:?}");
    }

    #[test]
    fn partial_handshake_is_dropped_too() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let ctx = ctx_with(vec![], 1);
        let srv = thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            handle_client(s, ctx);
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").unwrap(); // never finishes the request
        srv.join().unwrap();
    }

    #[test]
    fn dispatch_input_round_trips_through_real_pty() {
        let s = Session::new(
            0,
            "t".into(),
            vec!["sh".into(), "-c".into(), "exec cat".into()],
            &PtyConfig::default(),
            &RespawnConfig::default(),
        )
        .unwrap();
        let hub = ClientHub::new(256);
        let rx = hub.register();
        spawn_reader(&s, &hub, s.generation());
        let sessions = vec![s.clone()];
        let d = |raw: &str| dispatch(&decode_client(raw).unwrap(), &sessions, &hub);
        d(r#"{"type":"resize","session":0,"cols":80,"rows":24}"#);
        d(r#"{"type":"input","session":0,"data":"h\u00e9llo\n"}"#);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = false;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(m) = rx.recv_timeout(left) {
                if let Ok(ServerMsg::Output { session: 0, data }) = serde_json::from_str(&m) {
                    if data.contains("h\u{e9}llo") {
                        seen = true;
                        break;
                    }
                }
            }
        }
        assert!(seen);
        // Unknown session is ignored; unknown or malformed messages never decode.
        d(r#"{"type":"input","session":9,"data":"x"}"#);
        assert!(decode_client(r#"{"type":"bogus","session":0}"#).is_none());
        assert!(decode_client(r#"{"type":"input"}"#).is_none());
        assert_eq!(
            session_list_message(&sessions),
            ServerMsg::SessionList { sessions: vec![SessionInfo { index: 0, name: "t".into() }] }
        );
    }
}
