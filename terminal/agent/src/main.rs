//! terminal-agent: on-device PTY-to-WebSocket relay for the terminal feature.
//!
//! Rust port of the original `terminal-agent.py` (deleted; see docs/terminal.md).
//! Spawns one PTY-backed child process per entry in `terminal-agent-config.json`
//! (a local shell, or `ssh <host>`) and relays PTY I/O to any connected WebSocket
//! client (`terminal/frontend/`) on ws://0.0.0.0:8766, speaking the JSON wire
//! protocol documented in docs/terminal.md and extended by
//! docs/research/button-knob-clock-design.md sections 4 and 8:
//!
//!   agent -> frontend:
//!     {"type":"session_list","sessions":[{"index":0,"name":"...."}, ...]}
//!     {"type":"output","session":N,"data":"<decoded pty stdout chunk>"}
//!     {"type":"exited","session":N,"code":Option<i32>,
//!      "auto_respawning":bool,"needs_manual_respawn":bool}
//!     {"type":"clock_sync","offset_ms":<i64>,"status":"ok"|"stale"|"unsynced",
//!      "age_s":<u64, present when status != "ok">}
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::Message;

const DEFAULT_CONFIG_PATH: &str = "/opt/thing-revived/terminal-agent-config.json";
const PORT: u16 = 8766; // bridge/ already owns 8765; separate process, separate port.

/// Bounded auto-respawn window (design doc §8): up to 3 involuntary exits
/// within this many seconds get an automatic retry; the 4th within the
/// window stops auto-respawning and asks for a manual trigger instead.
const RESPAWN_WINDOW_S: u64 = 30;
const RESPAWN_MAX_AUTO_ATTEMPTS: usize = 3;
/// Delay before each successive auto-respawn attempt (1st, 2nd, 3rd),
/// avoiding a tight crash loop against e.g. a bad ssh command.
const RESPAWN_DELAYS_S: [u64; 3] = [0, 2, 5];

/// Clock sync (design doc §4).
const CLOCK_SYNC_INTERVAL_S: u64 = 60;
const CLOCK_SYNC_BACKOFF_S: [u64; 3] = [10, 20, 40];
const SSH_KEY_PATH: &str = "/var/lib/thing-revived/.ssh/id_ed25519";
const SSH_KNOWN_HOSTS_PATH: &str = "/var/lib/thing-revived/.ssh/known_hosts";
const CLOCK_SYNC_SSH_USER: &str = "andremmfaria";

#[derive(Deserialize)]
struct ConfigFile {
    sessions: Vec<SessionConfig>,
    /// Optional explicit override for vermelho's address - highest
    /// precedence in discover_vermelho_host(), for when the link topology
    /// changes (e.g. a future non-point-to-point network). See design doc
    /// §4 "Host discovery".
    vermelho_host: Option<String>,
}

#[derive(Deserialize)]
struct SessionConfig {
    name: Option<String>,
    command: Vec<String>,
}

fn load_config() -> (Vec<SessionConfig>, Option<String>) {
    let path = env::var("TERMINAL_AGENT_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("[terminal-agent] failed to read {path}: {e}"));
    let cfg: ConfigFile = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("[terminal-agent] failed to parse {path}: {e}"));
    if cfg.sessions.is_empty() {
        panic!("[terminal-agent] no sessions defined in {path}");
    }
    (cfg.sessions, cfg.vermelho_host)
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
    /// Timestamps of recent INVOLUNTARY exits only (detected via
    /// pump_session_output's EOF/EIO path) - a manual "control"/"respawn"
    /// never touches this, so a user repeatedly respawning by hand can
    /// never trip the auto-backoff limiter meant for crash-looping
    /// commands. Design doc §8.
    involuntary_exits: Mutex<Vec<Instant>>,
}

/// Opens a PTY pair and spawns `command` on the slave side, in its own
/// session/process group (`setsid`), matching the Python original's
/// `preexec_fn=os.setsid` - needed so "interrupt" can signal the whole
/// group, not just the immediate pid.
fn spawn_pty(command: &[String]) -> io::Result<(RawFd, Child)> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // SAFETY: `openpty` is a glibc/BSD extension; we pass null for
    // name/termios/winsize, matching Python's `pty.openpty()` defaults.
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
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            // Without this, the child becomes a session leader with NO
            // controlling terminal (setsid() alone doesn't grant one, and
            // we never open(2) the slave path here -- the fds are dup'd,
            // not freshly opened, so Linux's "open() of an unowned tty by
            // a ctty-less session leader" auto-attach never fires either).
            // Confirmed live: without this, a bare `resize` WS message
            // (ioctl TIOCSWINSZ on the master) produces zero PTY output --
            // no SIGWINCH ever reaches the child, because the kernel only
            // delivers it to a terminal's registered foreground process
            // group, and without a controlling terminal there is none.
            // This breaks two things: ssh (session 0) never forwards the
            // real window size to the remote glances process (which then
            // renders at its sshd-assigned fallback size, not the kiosk's
            // real width), and bash never redraws its prompt on resize.
            if libc::ioctl(slave, libc::TIOCSCTTY, 0) == -1 {
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
    /// Spawns one configured session's PTY. Returns Err (never panics) on
    /// spawn failure, e.g. a configured command that isn't present on this
    /// firmware (ssh client not installed yet, etc.) — a bad entry in one
    /// session's config must not take down every other session or the
    /// WebSocket server itself.
    fn new(index: usize, name: String, command: Vec<String>) -> Result<Arc<Session>, String> {
        let (master_fd, child) = spawn_pty(&command).map_err(|e| {
            format!("[terminal-agent] session {index} ({name}): spawn failed: {e}")
        })?;
        println!(
            "[terminal-agent] session {index} ({name}): spawned pid={} cmd={:?}",
            child.id(),
            command
        );
        Ok(Arc::new(Session {
            index,
            name,
            command,
            inner: Mutex::new(SessionInner {
                master_fd: Some(master_fd),
                child: Some(child),
            }),
            involuntary_exits: Mutex::new(Vec::new()),
        }))
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

    /// Records one involuntary exit now and returns how many have happened
    /// within the trailing RESPAWN_WINDOW_S window (including this one),
    /// pruning older entries. Design doc §8.
    fn record_involuntary_exit(&self) -> usize {
        let now = Instant::now();
        let mut exits = self.involuntary_exits.lock().unwrap();
        exits.retain(|t| now.duration_since(*t) <= Duration::from_secs(RESPAWN_WINDOW_S));
        exits.push(now);
        exits.len()
    }
}

type Clients = Arc<Mutex<Vec<Sender<String>>>>;

fn broadcast(clients: &Clients, msg: &Value) {
    let json = msg.to_string();
    let mut guard = clients.lock().unwrap();
    guard.retain(|tx| tx.send(json.clone()).is_ok());
}

/// Continuously reads one session's PTY master fd and broadcasts the
/// output, mirroring the Python original's `pump_session_output`. On the
/// child's exit (EOF/EIO), applies the bounded auto-respawn policy from
/// design doc §8: up to 3 involuntary exits within 30s get an automatic
/// retry (with increasing delay) and a fresh reader thread; the 4th in the
/// same window gives up and asks for a manual respawn via `needs_manual_respawn`.
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
        let err = io::Error::last_os_error();
        if n == 0 || (n < 0 && err.raw_os_error() == Some(libc::EIO)) {
            // Clean EOF, or EIO - the common PTY "slave closed" signal on
            // Linux; treat both as "child exited".
            handle_involuntary_exit(&session, &clients);
            break;
        }
        if err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        eprintln!("[terminal-agent] session {}: read error: {err}", session.index);
        thread::sleep(Duration::from_millis(200));
    }
}

/// Shared exit-handling path for both EOF (n==0) and EIO ("slave closed")
/// cases: record the involuntary exit, broadcast the (possibly augmented)
/// `exited` message, and either queue a bounded auto-respawn or give up and
/// flag the session for manual recovery.
fn handle_involuntary_exit(session: &Arc<Session>, clients: &Clients) {
    let code = session.exit_code();
    let attempt_count = session.record_involuntary_exit();

    if attempt_count <= RESPAWN_MAX_AUTO_ATTEMPTS {
        let delay_s = RESPAWN_DELAYS_S[attempt_count - 1];
        println!(
            "[terminal-agent] session {}: involuntary exit #{attempt_count} within {RESPAWN_WINDOW_S}s window, \
             auto-respawning in {delay_s}s",
            session.index
        );
        broadcast(
            clients,
            &json!({
                "type": "exited",
                "session": session.index,
                "code": code,
                "auto_respawning": true,
                "needs_manual_respawn": false,
            }),
        );
        let session = session.clone();
        let clients = clients.clone();
        thread::spawn(move || {
            if delay_s > 0 {
                thread::sleep(Duration::from_secs(delay_s));
            }
            session.respawn();
            // A fresh child needs a fresh reader thread - pump_session_output's
            // own loop just broke permanently on the old child's EOF/EIO.
            thread::spawn(move || pump_session_output(session, clients));
        });
    } else {
        eprintln!(
            "[terminal-agent] session {}: {attempt_count} involuntary exits within {RESPAWN_WINDOW_S}s \
             - crash-looping, not transient; giving up on auto-respawn, needs manual trigger",
            session.index
        );
        broadcast(
            clients,
            &json!({
                "type": "exited",
                "session": session.index,
                "code": code,
                "auto_respawning": false,
                "needs_manual_respawn": true,
            }),
        );
    }
}

fn dispatch(msg: &Value, sessions: &[Arc<Session>], clients: &Clients) {
    let mtype = msg.get("type").and_then(Value::as_str).unwrap_or("");
    let idx = match msg.get("session").and_then(Value::as_u64) {
        Some(i) => i as usize,
        None => return,
    };
    // Look up by each session's own `index` field, not raw vector
    // position: sessions that failed to spawn are absent from this slice
    // entirely (see main()'s filter_map), so vector position and the
    // configured `index` can diverge once any session is dropped.
    let session = match sessions.iter().find(|s| s.index == idx) {
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
            Some("respawn") => {
                // Manual respawn (user-triggered) deliberately does NOT touch
                // involuntary_exits - repeated manual respawns must never trip
                // the auto-backoff limiter meant for crash-looping commands.
                session.respawn();
                // pump_session_output's own thread already exited (it
                // breaks permanently on the old child's EOF, by design,
                // per its own doc comment) — a fresh child needs a fresh
                // reader thread, or its output would vanish into nothing
                // exactly like it did before this fix, confirmed on real
                // hardware: respawn succeeded but no output ever reached
                // the frontend again until this spawn was added.
                let session = session.clone();
                let clients = clients.clone();
                thread::spawn(move || pump_session_output(session, clients));
            }
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

// --- Clock sync (design doc §4) --------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ClockStatus {
    Unsynced,
    Ok,
    Stale,
}

impl ClockStatus {
    fn as_str(&self) -> &'static str {
        match self {
            ClockStatus::Unsynced => "unsynced",
            ClockStatus::Ok => "ok",
            ClockStatus::Stale => "stale",
        }
    }
}

struct ClockState {
    offset_ms: i64,
    status: ClockStatus,
    last_sync: Option<Instant>,
}

fn clock_sync_message(clock: &Mutex<ClockState>) -> Value {
    let state = clock.lock().unwrap();
    let mut msg = json!({
        "type": "clock_sync",
        "offset_ms": state.offset_ms,
        "status": state.status.as_str(),
    });
    if state.status != ClockStatus::Ok {
        let age_s = state.last_sync.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        msg["age_s"] = json!(age_s);
    }
    msg
}

/// Discovers vermelho's current address, highest precedence first:
/// 1. `TERMINAL_AGENT_VERMELHO_HOST` env var.
/// 2. `vermelho_host` key in terminal-agent-config.json.
/// 3. The gateway of the device's own default route (`ip route show
///    default`), re-resolved every cycle so a lease change is picked up
///    without a restart - this is the mechanism the design doc specifies,
///    valid if/when this link ever has an actual default-route gateway.
/// 4. Fallback for THIS link's actual live topology (confirmed via `ip
///    route` on-device): the USB-CDC-NCM point-to-point link currently has
///    no default route at all, just a directly-connected /29 scope-link
///    subnet with no `via` gateway - `ip route show default` returns
///    nothing to parse. Rather than hardcode vermelho's address (explicitly
///    disallowed), derive the subnet from the device's own primary
///    non-loopback IPv4 address + prefix and TCP-probe port 22 on each
///    other host in that small range (a /29 has only 6 usable addresses),
///    returning the first that answers. This still re-resolves to whatever
///    address is actually reachable on every cycle, so it survives a lease
///    change exactly like the default-route path would, just via a
///    different signal given this topology's real constraints.
fn discover_vermelho_host(config_override: &Option<String>) -> Option<String> {
    if let Ok(h) = env::var("TERMINAL_AGENT_VERMELHO_HOST") {
        if !h.is_empty() {
            return Some(h);
        }
    }
    if let Some(h) = config_override {
        if !h.is_empty() {
            return Some(h.clone());
        }
    }
    if let Some(h) = default_route_gateway() {
        return Some(h);
    }
    subnet_probe_fallback()
}

fn default_route_gateway() -> Option<String> {
    let out = Command::new("ip").args(["route", "show", "default"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Expected form: "default via 10.42.1.30 dev uncm0 ..."
    let mut parts = text.split_whitespace();
    while let Some(tok) = parts.next() {
        if tok == "via" {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}

/// Parses `ip -4 addr show` for the device's own primary non-loopback
/// address + prefix length, then TCP-probes port 22 on every other host in
/// that subnet, returning the first that accepts a connection. Bounded to
/// small subnets (/28 or smaller, i.e. <=16 hosts) to keep this fast and
/// avoid turning into an accidental port scanner on a larger network.
fn subnet_probe_fallback() -> Option<String> {
    let out = Command::new("ip").args(["-4", "-o", "addr", "show"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains(" lo ") || line.trim_start().starts_with("1:") {
            continue; // skip loopback
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let cidr = parts.iter().find(|p| p.contains('/'))?;
        let (addr_str, prefix_str) = cidr.split_once('/')?;
        let prefix: u32 = prefix_str.parse().ok()?;
        if prefix < 28 {
            continue; // subnet too large to probe cheaply; skip
        }
        let own: std::net::Ipv4Addr = addr_str.parse().ok()?;
        let own_u32 = u32::from(own);
        let mask: u32 = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
        let network = own_u32 & mask;
        let host_bits = 32 - prefix;
        let host_count = 1u32 << host_bits;
        for i in 1..host_count.saturating_sub(1) {
            let candidate_u32 = network | i;
            if candidate_u32 == own_u32 {
                continue;
            }
            let candidate = std::net::Ipv4Addr::from(candidate_u32);
            if TcpStream::connect_timeout(
                &std::net::SocketAddr::new(candidate.into(), 22),
                Duration::from_millis(300),
            )
            .is_ok()
            {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

/// Runs `ssh ... date +%s%3N` against `host`, returning the RTT-corrected
/// offset in ms (remote - local), or None on any failure (bad exit,
/// timeout, unparseable output). Design doc §4 "Sync mechanism".
fn probe_offset_ms(host: &str) -> Option<i64> {
    let t0 = SystemTime::now();
    let output = Command::new("ssh")
        .args([
            "-i",
            SSH_KEY_PATH,
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("UserKnownHostsFile={SSH_KNOWN_HOSTS_PATH}"),
            &format!("{CLOCK_SYNC_SSH_USER}@{host}"),
            "date +%s%3N",
        ])
        .output()
        .ok()?;
    let t1 = SystemTime::now();
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let remote_epoch_ms: i64 = text.trim().parse().ok()?;

    let rtt_ms = t1.duration_since(t0).ok()?.as_millis() as i64;
    let local_epoch_ms_at_t1 = t1.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;

    let remote_estimate_ms = remote_epoch_ms + rtt_ms / 2;
    Some(remote_estimate_ms - local_epoch_ms_at_t1)
}

/// Dedicated thread: periodically re-discovers vermelho's host, probes the
/// offset, and broadcasts clock_sync to all connected clients. Backs off on
/// repeated failure (10s/20s/40s) instead of hammering an unreachable host,
/// and never zeroes a previously-good offset on transient failure - only
/// the status flips to "stale". Design doc §4 "Failure handling".
fn clock_sync_loop(clients: Clients, clock: Arc<Mutex<ClockState>>, vermelho_override: Option<String>) {
    let mut consecutive_failures: usize = 0;
    loop {
        let host = discover_vermelho_host(&vermelho_override);
        let result = host.as_deref().and_then(probe_offset_ms);

        match result {
            Some(offset_ms) => {
                consecutive_failures = 0;
                let mut state = clock.lock().unwrap();
                state.offset_ms = offset_ms;
                state.status = ClockStatus::Ok;
                state.last_sync = Some(Instant::now());
                drop(state);
                println!(
                    "[terminal-agent] clock_sync: ok, host={}, offset_ms={offset_ms}",
                    host.as_deref().unwrap_or("?")
                );
            }
            None => {
                consecutive_failures += 1;
                let mut state = clock.lock().unwrap();
                if state.last_sync.is_some() {
                    state.status = ClockStatus::Stale;
                } // else stays Unsynced - never had a good offset at all
                drop(state);
                eprintln!(
                    "[terminal-agent] clock_sync: probe failed (host={:?}, consecutive_failures={consecutive_failures})",
                    host
                );
            }
        }

        broadcast(&clients, &clock_sync_message(&clock));

        let sleep_s = if consecutive_failures == 0 {
            CLOCK_SYNC_INTERVAL_S
        } else {
            let idx = (consecutive_failures - 1).min(CLOCK_SYNC_BACKOFF_S.len() - 1);
            CLOCK_SYNC_BACKOFF_S[idx]
        };
        thread::sleep(Duration::from_secs(sleep_s));
    }
}

// --- WebSocket server -------------------------------------------------------

/// One OS thread per client connection. Reading and writing a single sync
/// `tungstenite::WebSocket` concurrently needs two independent handles on
/// the same socket: the handshake accept() keeps the original `TcpStream`
/// for the blocking read loop, and a `try_clone()`'d duplicate (sharing the
/// same underlying socket, a distinct fd) backs a second `WebSocket` used
/// only for writing broadcast messages from a dedicated writer thread. This
/// keeps PTY output from ever blocking on, or being blocked by, reads.
fn handle_client(
    stream: TcpStream,
    sessions: Arc<Vec<Arc<Session>>>,
    clients: Clients,
    clock: Arc<Mutex<ClockState>>,
) {
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
    // So a freshly opened tab doesn't show "unsynced" for a full sync
    // interval - send the current cached clock state immediately too.
    broadcast(&clients, &clock_sync_message(&clock));

    thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(json) => {
                if ws_write.send(Message::Text(json.into())).is_err() {
                    break;
                }
                let _ = ws_write.flush();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if ws_write.send(Message::Ping(Vec::new().into())).is_err() {
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
                    dispatch(&msg, &sessions, &clients);
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
    let (configs, vermelho_override) = load_config();
    let sessions: Vec<Arc<Session>> = configs
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| {
            match Session::new(i, c.name.unwrap_or_else(|| format!("session-{i}")), c.command) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("{e} (session skipped, others unaffected)");
                    None
                }
            }
        })
        .collect();
    if sessions.is_empty() {
        eprintln!("[terminal-agent] warning: zero sessions spawned successfully, WebSocket server will start with nothing to serve");
    }
    let sessions = Arc::new(sessions);
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    for session in sessions.iter() {
        let session = session.clone();
        let clients = clients.clone();
        thread::spawn(move || pump_session_output(session, clients));
    }

    let clock = Arc::new(Mutex::new(ClockState {
        offset_ms: 0,
        status: ClockStatus::Unsynced,
        last_sync: None,
    }));
    {
        let clients = clients.clone();
        let clock = clock.clone();
        thread::spawn(move || clock_sync_loop(clients, clock, vermelho_override));
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
                let clock = clock.clone();
                thread::spawn(move || handle_client(stream, sessions, clients, clock));
            }
            Err(e) => eprintln!("[terminal-agent] accept error: {e}"),
        }
    }
}
