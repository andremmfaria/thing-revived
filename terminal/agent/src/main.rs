//! terminal-agent: on-device PTY-to-WebSocket relay for the terminal feature.
//!
//! Rust port of the original `terminal-agent.py` (deleted; see docs/terminal.md).
//! Spawns one PTY-backed child process per entry in `terminal-agent-config.json`
//! (a local shell, or `ssh <host>`) and relays PTY I/O to any connected WebSocket
//! client (`terminal/frontend/`) on ws://0.0.0.0:8766, speaking the exact same
//! JSON wire protocol documented in docs/terminal.md:
//!
//!   agent -> frontend:
//!     {"type":"session_list","sessions":[{"index":0,"name":"..."}, ...]}
//!     {"type":"output","session":N,"data":"<decoded pty stdout chunk>"}
//!     {"type":"exited","session":N,"code":N}
//!   frontend -> agent:
//!     {"type":"input","session":N,"data":"<raw keystrokes>"}
//!     {"type":"resize","session":N,"cols":C,"rows":R}
//!     {"type":"control","session":N,"action":"interrupt"|"clear"|"respawn"}
//!
//! Unlike the Python original (which hand-rolled a minimal RFC 6455 server
//! because the stdlib has none), this reuses `tungstenite`, the same
//! WebSocket crate bridge/ already depends on, sync/threaded rather than
//! bridge/'s own async choice elsewhere, since bridge/ itself is sync
//! (`tungstenite::accept` + one OS thread per client), not tokio-based.

use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::Message;

const DEFAULT_CONFIG_PATH: &str = "/opt/thing-revived/terminal-agent-config.json";
const PORT: u16 = 8766; // bridge/ already owns 8765; separate process, separate port.

#[derive(Deserialize)]
struct ConfigFile {
    sessions: Vec<SessionConfig>,
}

#[derive(Deserialize)]
struct SessionConfig {
    name: Option<String>,
    command: Vec<String>,
}

fn load_config() -> Vec<SessionConfig> {
    let path = env::var("TERMINAL_AGENT_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("[terminal-agent] failed to read {path}: {e}"));
    let cfg: ConfigFile = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("[terminal-agent] failed to parse {path}: {e}"));
    if cfg.sessions.is_empty() {
        panic!("[terminal-agent] no sessions defined in {path}");
    }
    cfg.sessions
}

/// Mutable state behind one PTY-backed child process.
struct SessionInner {
    master_fd: Option<RawFd>,
    child: Option<Child>,
}

struct Session {
    index: usize,
    name: String,
    command: Vec<String>,
    inner: Mutex<SessionInner>,
}

/// Opens a PTY pair and spawns `command` on the slave side, in its own
/// session/process group (`setsid`), matching the Python original's
/// `preexec_fn=os.setsid` - needed so "interrupt" can signal the whole
/// group, not just the immediate pid.
fn spawn_pty(command: &[String]) -> io::Result<(RawFd, Child)> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // SAFETY: `openpty` is a glibc/BSD extension (present on our glibc
    // `gnueabihf` target); we pass null for name/termios/winsize, matching
    // Python's `pty.openpty()` defaults.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    // The master fd must not leak into the child (it would keep the PTY's
    // read side open from two processes). Python gets this via
    // `subprocess`'s default `close_fds=True`; set FD_CLOEXEC explicitly
    // here since Rust's `Command` does not close arbitrary inherited fds.
    unsafe {
        libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
    }

    let prog = &command[0];
    let args = &command[1..];
    let mut cmd = Command::new(prog);
    cmd.args(args);

    // Three independent dup()s of the slave fd for stdin/stdout/stderr -
    // same as Python passing `stdin=stdout=stderr=slave_fd` into Popen,
    // just made explicit since Rust's Stdio::from_raw_fd takes ownership
    // of (and will close) whatever fd it is given.
    let dup_slave = |fd: RawFd| -> io::Result<RawFd> {
        let d = unsafe { libc::dup(fd) };
        if d < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(d)
        }
    };
    let stdin_fd = dup_slave(slave)?;
    let stdout_fd = dup_slave(slave)?;
    // stderr takes the original slave fd (no extra dup needed, matches the
    // "N dup'd copies + 1 original" shape, all closed by Command after spawn).
    unsafe {
        cmd.stdin(Stdio::from_raw_fd(stdin_fd));
        cmd.stdout(Stdio::from_raw_fd(stdout_fd));
        cmd.stderr(Stdio::from_raw_fd(slave));
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn().inspect_err(|_| unsafe {
        libc::close(master);
    })?;

    Ok((master, child))
}

impl Session {
    fn new(index: usize, name: String, command: Vec<String>) -> Arc<Session> {
        let (master_fd, child) = spawn_pty(&command)
            .unwrap_or_else(|e| panic!("[terminal-agent] session {index} ({name}): spawn failed: {e}"));
        println!(
            "[terminal-agent] session {index} ({name}): spawned pid={} cmd={:?}",
            child.id(),
            command
        );
        Arc::new(Session {
            index,
            name,
            command,
            inner: Mutex::new(SessionInner {
                master_fd: Some(master_fd),
                child: Some(child),
            }),
        })
    }

    fn write(&self, data: &str) {
        let fd = match self.inner.lock().unwrap().master_fd {
            Some(fd) => fd,
            None => return,
        };
        let bytes = data.as_bytes();
        let rc = unsafe { libc::write(fd, bytes.as_ptr() as *const _, bytes.len()) };
        if rc < 0 {
            eprintln!(
                "[terminal-agent] session {}: write failed: {}",
                self.index,
                io::Error::last_os_error()
            );
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        let fd = match self.inner.lock().unwrap().master_fd {
            Some(fd) => fd,
            None => return,
        };
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
        if rc != 0 {
            eprintln!(
                "[terminal-agent] session {}: resize failed: {}",
                self.index,
                io::Error::last_os_error()
            );
        }
    }

    /// SIGINT to the whole process group. The child called `setsid()`, so
    /// its process group id equals its own pid; negative pid in `kill(2)`
    /// targets the whole group, same effect as Python's
    /// `os.killpg(os.getpgid(pid), SIGINT)`.
    fn interrupt(&self) {
        let pid = match &self.inner.lock().unwrap().child {
            Some(c) => c.id() as i32,
            None => return,
        };
        unsafe {
            libc::kill(-pid, libc::SIGINT);
        }
    }

    /// Ctrl-L: same byte a human would type, not a true terminal reset -
    /// documented limitation, same as the Python original.
    fn clear(&self) {
        self.write("\x0c");
    }

    fn respawn(&self) {
        let (old_pid, old_fd) = {
            let mut inner = self.inner.lock().unwrap();
            let pid = inner.child.as_ref().map(|c| c.id() as i32);
            inner.child = None;
            (pid, inner.master_fd.take())
        };
        if let Some(pid) = old_pid {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        if let Some(fd) = old_fd {
            unsafe {
                libc::close(fd);
            }
        }
        match spawn_pty(&self.command) {
            Ok((master_fd, child)) => {
                println!(
                    "[terminal-agent] session {} ({}): respawned pid={}",
                    self.index,
                    self.name,
                    child.id()
                );
                let mut inner = self.inner.lock().unwrap();
                inner.master_fd = Some(master_fd);
                inner.child = Some(child);
            }
            Err(e) => {
                eprintln!("[terminal-agent] session {}: respawn failed: {e}", self.index);
            }
        }
    }

    /// Current exit code if the child has exited, without blocking.
    fn exit_code(&self) -> Option<i32> {
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.child {
            Some(c) => match c.try_wait() {
                Ok(Some(status)) => status.code(),
                _ => None,
            },
            None => None,
        }
    }
}

type Clients = Arc<Mutex<Vec<Sender<String>>>>;

fn broadcast(clients: &Clients, msg: &Value) {
    let json = msg.to_string();
    let mut guard = clients.lock().unwrap();
    guard.retain(|tx| tx.send(json.clone()).is_ok());
}

/// Continuously reads one session's PTY master fd and broadcasts the output,
/// mirroring the Python original's `pump_session_output` (there via
/// `select.select` + `os.read`; here via a single blocking `read(2)` per
/// loop iteration, since each session owns its own OS thread).
fn pump_session_output(session: Arc<Session>, clients: Clients) {
    let mut buf = [0u8; 4096];
    loop {
        let fd = match session.inner.lock().unwrap().master_fd {
            Some(fd) => fd,
            None => break,
        };
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            let chunk = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
            broadcast(
                &clients,
                &json!({"type": "output", "session": session.index, "data": chunk}),
            );
            continue;
        }
        if n == 0 {
            // Child exited and closed its end. Report it, then wait for a
            // manual "respawn" control message rather than looping forever.
            let code = session.exit_code();
            broadcast(
                &clients,
                &json!({"type": "exited", "session": session.index, "code": code}),
            );
            break;
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if err.raw_os_error() == Some(libc::EIO) {
            // Common PTY "slave closed" signal on Linux, treat like EOF.
            let code = session.exit_code();
            broadcast(
                &clients,
                &json!({"type": "exited", "session": session.index, "code": code}),
            );
            break;
        }
        eprintln!("[terminal-agent] session {}: read error: {err}", session.index);
        thread::sleep(Duration::from_millis(200));
    }
}

fn dispatch(msg: &Value, sessions: &[Arc<Session>]) {
    let mtype = msg.get("type").and_then(Value::as_str).unwrap_or("");
    let idx = match msg.get("session").and_then(Value::as_u64) {
        Some(i) => i as usize,
        None => return,
    };
    let session = match sessions.get(idx) {
        Some(s) => s,
        None => return,
    };
    match mtype {
        "input" => {
            if let Some(data) = msg.get("data").and_then(Value::as_str) {
                session.write(data);
            }
        }
        "resize" => {
            let cols = msg.get("cols").and_then(Value::as_u64);
            let rows = msg.get("rows").and_then(Value::as_u64);
            if let (Some(cols), Some(rows)) = (cols, rows) {
                session.resize(cols as u16, rows as u16);
            }
        }
        "control" => match msg.get("action").and_then(Value::as_str) {
            Some("interrupt") => session.interrupt(),
            Some("clear") => session.clear(),
            Some("respawn") => session.respawn(),
            _ => {}
        },
        _ => {}
    }
}

fn session_list_message(sessions: &[Arc<Session>]) -> Value {
    json!({
        "type": "session_list",
        "sessions": sessions
            .iter()
            .map(|s| json!({"index": s.index, "name": s.name}))
            .collect::<Vec<_>>(),
    })
}

/// One OS thread per client connection. Reading and writing a single sync
/// `tungstenite::WebSocket` concurrently needs two independent handles on
/// the same socket: the handshake accept() keeps the original `TcpStream`
/// for the blocking read loop, and a `try_clone()`'d duplicate (sharing the
/// same underlying socket, a distinct fd) backs a second `WebSocket` used
/// only for writing broadcast messages from a dedicated writer thread. This
/// keeps PTY output from ever blocking on, or being blocked by, reads.
fn handle_client(stream: TcpStream, sessions: Arc<Vec<Arc<Session>>>, clients: Clients) {
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[terminal-agent] failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_read = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[terminal-agent] websocket handshake failed: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    let (tx, rx) = channel::<String>();
    clients.lock().unwrap().push(tx);
    println!("[terminal-agent] frontend client connected");
    broadcast(&clients, &session_list_message(&sessions));

    thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(json) => {
                if ws_write.send(Message::Text(json)).is_err() {
                    break;
                }
                let _ = ws_write.flush();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if ws_write.send(Message::Ping(Vec::new())).is_err() {
                    break;
                }
                let _ = ws_write.flush();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    });

    loop {
        match ws_read.read() {
            Ok(Message::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<Value>(&text) {
                    dispatch(&msg, &sessions);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    println!("[terminal-agent] frontend client disconnected");
    // The writer thread's channel sender still lives in `clients` (same
    // lazy-prune-on-send-failure pattern bridge/ uses); it will notice the
    // closed socket on its own next write attempt and exit.
}

fn main() {
    let configs = load_config();
    let sessions: Vec<Arc<Session>> = configs
        .into_iter()
        .enumerate()
        .map(|(i, c)| Session::new(i, c.name.unwrap_or_else(|| format!("session-{i}")), c.command))
        .collect();
    let sessions = Arc::new(sessions);
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    for session in sessions.iter() {
        let session = session.clone();
        let clients = clients.clone();
        thread::spawn(move || pump_session_output(session, clients));
    }

    let listener = TcpListener::bind(("0.0.0.0", PORT)).expect("bind 0.0.0.0:8766 failed");
    println!("[terminal-agent] WebSocket server listening on ws://0.0.0.0:{PORT}");
    println!(
        "[terminal-agent] sessions: {:?}",
        sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let sessions = sessions.clone();
                let clients = clients.clone();
                thread::spawn(move || handle_client(stream, sessions, clients));
            }
            Err(e) => eprintln!("[terminal-agent] accept error: {e}"),
        }
    }
}
