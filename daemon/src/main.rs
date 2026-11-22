//! terminal-agent binary (see lib.rs). Wiring is being moved out of this file.

use terminal_agent::protocol::{decode_client, encode, ClientMsg, ClockStatus, ControlAction, ServerMsg, SessionInfo};
use terminal_agent::{hub, net, utf8};

use serde::Deserialize;
use std::env;
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::Message;

const DEFAULT_CONFIG_PATH: &str = "/opt/thing-revived/terminal-agent-config.json";

/// Bounded auto-respawn window (design doc §8): up to 3 involuntary exits
/// within this many seconds get an automatic retry; the 4th within the
/// window stops auto-respawning and asks for a manual trigger instead.
/// How often an idle reader re-checks whether its generation is stale.
const READER_POLL_MS: i32 = 200;
/// Upper bound on one blocked socket write to a client before it is dropped.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
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

fn parse_config(raw: &str) -> Result<ConfigFile, String> {
    let cfg: ConfigFile = serde_json::from_str(raw).map_err(|e| format!("failed to parse: {e}"))?;
    if cfg.sessions.is_empty() {
        return Err("no sessions defined".to_string());
    }
    if let Some(i) = cfg.sessions.iter().position(|s| s.command.is_empty()) {
        return Err(format!("session {i} has an empty command"));
    }
    Ok(cfg)
}

fn load_config() -> (Vec<SessionConfig>, Option<String>) {
    let path = env::var("TERMINAL_AGENT_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("[terminal-agent] failed to read {path}: {e}"));
    let cfg = parse_config(&raw).unwrap_or_else(|e| panic!("[terminal-agent] {path}: {e}"));
    (cfg.sessions, cfg.vermelho_host)
}

/// Mutable state behind one PTY-backed child process.
struct SessionInner {
    master_fd: Option<RawFd>,
    child: Option<Child>,
    /// Bumped on every (re)spawn. A reader thread carries the generation it
    /// was started for and exits as soon as this no longer matches, so a
    /// manual respawn can never leave two readers on the new fd, nor have the
    /// old reader's EOF counted as an involuntary exit of the new child.
    generation: u64,
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
    /// Serializes respawns (manual vs delayed auto) so two spawns can never
    /// race to install a child. Held across spawn_pty, never with `inner`.
    respawn_lock: Mutex<()>,
}

/// Opens a PTY pair and spawns `command` on the slave side, in its own
/// session/process group (`setsid`), matching the Python original's
/// `preexec_fn=os.setsid` - needed so "interrupt" can signal the whole
/// group, not just the immediate pid.
fn spawn_pty(command: &[String]) -> io::Result<(RawFd, Child)> {
    // Serialize the openpty..spawn window process-wide. Without this a
    // concurrent spawn (another session, or a respawn) can fork while this
    // call's slave fds are open and non-CLOEXEC, inherit them, and keep the
    // slave open so this session never sees EOF when its child exits.
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());
    let _spawn_guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        // Same for the slave original: Command dup2()s the stdio copies onto
        // 0/1/2 (clearing CLOEXEC there), so only the stray original closes.
        libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
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
        let d = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
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
                generation: 1,
            }),
            involuntary_exits: Mutex::new(Vec::new()),
            respawn_lock: Mutex::new(()),
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

    fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }

    /// Kills the old process group and reaps the old child, then spawns a
    /// fresh one. Returns the new generation to start a reader for, or None
    /// if the spawn failed. The generation is bumped before anything else so
    /// the old reader goes stale immediately.
    fn respawn(&self) -> Option<u64> {
        self.respawn_if(None)
    }

    /// Like `respawn`, but when `only_if_generation` is set, does nothing if
    /// the session was respawned (e.g. manually) since that generation. The
    /// delayed auto-respawn uses this so it cannot clobber a newer child.
    fn respawn_if(&self, only_if_generation: Option<u64>) -> Option<u64> {
        let _serialize = self.respawn_lock.lock().unwrap();
        let (old_child, old_fd) = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(g) = only_if_generation {
                if inner.generation != g {
                    return None;
                }
            }
            inner.generation += 1;
            (inner.child.take(), inner.master_fd.take())
        };
        if let Some(mut child) = old_child {
            // The child is a session leader (setsid), so its pid is the pgid.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            // SIGKILL makes this return promptly; a thread keeps an
            // uninterruptible child from ever stalling the caller.
            thread::spawn(move || {
                let _ = child.wait();
            });
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
                Some(inner.generation)
            }
            Err(e) => {
                eprintln!("[terminal-agent] session {}: respawn failed: {e}", self.index);
                None
            }
        }
    }

    /// A private dup of the master fd for the reader of `generation`, or None
    /// if that generation is stale. Owning a dup means the number cannot be
    /// closed and reused by another session while the reader is mid-read.
    fn reader_fd(&self, generation: u64) -> Option<OwnedFd> {
        let inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return None;
        }
        let fd = inner.master_fd?;
        let d = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if d < 0 {
            None
        } else {
            Some(unsafe { OwnedFd::from_raw_fd(d) })
        }
    }

    /// Exit code of the child of `generation`, reaping it. Outer None means
    /// the generation is stale (respawned meanwhile); inner None means the
    /// child could not be reaped yet. try_wait is polled briefly because EOF
    /// on the master can arrive a moment before the child is reapable.
    fn reap_exit(&self, generation: u64) -> Option<Option<i32>> {
        for attempt in 0..20 {
            {
                let mut inner = self.inner.lock().unwrap();
                if inner.generation != generation {
                    return None;
                }
                match &mut inner.child {
                    Some(c) => {
                        if let Ok(Some(status)) = c.try_wait() {
                            return Some(status.code());
                        }
                    }
                    None => return Some(None),
                }
            }
            if attempt < 19 {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Some(None)
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

use hub::Clients;

fn output_message(session: usize, data: &str) -> ServerMsg {
    ServerMsg::Output { session, data: data.to_string() }
}

fn exited_message(session: usize, code: Option<i32>, auto_respawning: bool) -> ServerMsg {
    ServerMsg::Exited { session, code, auto_respawning, needs_manual_respawn: !auto_respawning }
}

fn broadcast(clients: &Clients, msg: &ServerMsg) {
    let dropped = hub::broadcast_str(clients, &encode(msg));
    if dropped > 0 {
        eprintln!("[terminal-agent] dropped {dropped} stalled client(s): queue full");
    }
}

/// Continuously reads one session's PTY master fd and broadcasts the
/// output, mirroring the Python original's `pump_session_output`. On the
/// child's exit (EOF/EIO), applies the bounded auto-respawn policy from
/// design doc §8: up to 3 involuntary exits within 30s get an automatic
/// retry (with increasing delay) and a fresh reader thread; the 4th in the
/// same window gives up and asks for a manual respawn via `needs_manual_respawn`.
fn pump_session_output(session: Arc<Session>, clients: Clients, generation: u64) {
    let fd = match session.reader_fd(generation) {
        Some(fd) => fd,
        None => return,
    };
    let raw = fd.as_raw_fd();
    let mut buf = [0u8; 4096];
    let mut carry = utf8::Utf8Carry::new();
    loop {
        // Wake periodically so a reader whose generation went stale (manual
        // respawn) exits even if its old PTY never signals EOF.
        let mut pfd = libc::pollfd {
            fd: raw,
            events: libc::POLLIN,
            revents: 0,
        };
        let pr = unsafe { libc::poll(&mut pfd, 1, READER_POLL_MS) };
        if pr == 0 {
            if session.generation() != generation {
                return;
            }
            continue;
        }
        if pr < 0 {
            if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                thread::sleep(Duration::from_millis(200));
            }
            continue;
        }
        let n = unsafe { libc::read(raw, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            if session.generation() != generation {
                return;
            }
            let chunk = carry.push(&buf[..n as usize]);
            if chunk.is_empty() {
                continue;
            }
            broadcast(&clients, &output_message(session.index, &chunk));
            continue;
        }
        let err = io::Error::last_os_error();
        if n == 0 || (n < 0 && err.raw_os_error() == Some(libc::EIO)) {
            // Clean EOF, or EIO - the common PTY "slave closed" signal on
            // Linux; treat both as "child exited". Stale generations (this
            // was a manual respawn) exit silently and are not counted.
            if session.generation() != generation {
                return;
            }
            let tail = carry.finish();
            if !tail.is_empty() {
                broadcast(&clients, &output_message(session.index, &tail));
            }
            handle_involuntary_exit(&session, &clients, generation);
            return;
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
fn handle_involuntary_exit(session: &Arc<Session>, clients: &Clients, generation: u64) {
    let code = match session.reap_exit(generation) {
        Some(code) => code,
        None => return, // respawned while we were waiting: not an involuntary exit
    };
    let attempt_count = session.record_involuntary_exit();

    if attempt_count <= RESPAWN_MAX_AUTO_ATTEMPTS {
        let delay_s = RESPAWN_DELAYS_S[attempt_count - 1];
        println!(
            "[terminal-agent] session {}: involuntary exit #{attempt_count} within {RESPAWN_WINDOW_S}s window, \
             auto-respawning in {delay_s}s",
            session.index
        );
        broadcast(clients, &exited_message(session.index, code, true));
        let session = session.clone();
        let clients = clients.clone();
        thread::spawn(move || {
            if delay_s > 0 {
                thread::sleep(Duration::from_secs(delay_s));
            }
            // A fresh child needs a fresh reader thread - the old reader
            // returned on the old child's EOF/EIO. Skipped if a manual
            // respawn already replaced the child during the delay.
            if let Some(g) = session.respawn_if(Some(generation)) {
                thread::spawn(move || pump_session_output(session, clients, g));
            }
        });
    } else {
        eprintln!(
            "[terminal-agent] session {}: {attempt_count} involuntary exits within {RESPAWN_WINDOW_S}s \
             - crash-looping, not transient; giving up on auto-respawn, needs manual trigger",
            session.index
        );
        broadcast(clients, &exited_message(session.index, code, false));
    }
}

fn dispatch(msg: &ClientMsg, sessions: &[Arc<Session>], clients: &Clients) {
    let idx = match msg {
        ClientMsg::Input { session, .. }
        | ClientMsg::Resize { session, .. }
        | ClientMsg::Control { session, .. } => *session as usize,
    };
    // Look up by each session's own `index` field, not raw vector
    // position: sessions that failed to spawn are absent from this slice
    // entirely (see main()'s filter_map), so vector position and the
    // configured `index` can diverge once any session is dropped.
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
                // involuntary_exits - repeated manual respawns must never trip
                // the auto-backoff limiter meant for crash-looping commands.
                // A fresh child needs a fresh reader thread, or its output
                // would vanish (confirmed on real hardware before the reader
                // was spawned here). The old reader goes stale via the
                // generation bump inside respawn and exits on its own.
                if let Some(g) = session.respawn() {
                    let session = session.clone();
                    let clients = clients.clone();
                    thread::spawn(move || pump_session_output(session, clients, g));
                }
            }
        },
    }
}

fn session_list_message(sessions: &[Arc<Session>]) -> ServerMsg {
    ServerMsg::SessionList {
        sessions: sessions.iter().map(|s| SessionInfo { index: s.index, name: s.name.clone() }).collect(),
    }
}

// --- Clock sync (design doc §4) --------------------------------------------

struct ClockState {
    offset_ms: i64,
    status: ClockStatus,
    last_sync: Option<Instant>,
}

fn clock_sync_message(clock: &Mutex<ClockState>) -> ServerMsg {
    let state = clock.lock().unwrap();
    let age_s = if state.status != ClockStatus::Ok {
        Some(state.last_sync.map(|t| t.elapsed().as_secs()).unwrap_or(0))
    } else {
        None
    };
    ServerMsg::ClockSync { offset_ms: state.offset_ms, status: state.status, age_s }
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

/// Handshake callback: rejects browser pages from foreign origins (see
/// net::origin_allowed for the accepted set) with a 403 before upgrading.
/// The large Err type is dictated by tungstenite's callback signature.
#[allow(clippy::result_large_err)]
fn check_origin(
    req: &tungstenite::handshake::server::Request,
    resp: tungstenite::handshake::server::Response,
) -> Result<
    tungstenite::handshake::server::Response,
    tungstenite::handshake::server::ErrorResponse,
> {
    let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if net::origin_allowed(origin) {
        Ok(resp)
    } else {
        eprintln!("[terminal-agent] rejected websocket handshake, origin {origin:?}");
        let mut err = tungstenite::handshake::server::ErrorResponse::new(Some("forbidden origin".to_string()));
        *err.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
        Err(err)
    }
}

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
    let mut ws_read = match tungstenite::accept_hdr(stream, check_origin) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[terminal-agent] websocket handshake failed: {e}");
            return;
        }
    };
    let write_stream_for_timeout = match write_stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[terminal-agent] failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    // A stalled peer must not wedge the writer thread forever either.
    let _ = write_stream_for_timeout.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT));
    let rx = hub::register(&clients);
    println!("[terminal-agent] frontend client connected");
    broadcast(&clients, &session_list_message(&sessions));
    // So a freshly opened tab doesn't show "unsynced" for a full sync
    // interval - send the current cached clock state immediately too.
    broadcast(&clients, &clock_sync_message(&clock));

    thread::spawn(move || {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(json) => {
                    if ws_write.send(Message::Text(json.to_string().into())).is_err() {
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
        }
        // Dropped for being slow, or the socket died: close it so the reader
        // loop below ends too and the client can reconnect cleanly.
        let _ = write_stream_for_timeout.shutdown(std::net::Shutdown::Both);
    });

    loop {
        match ws_read.read() {
            Ok(Message::Text(text)) => {
                if let Some(msg) = decode_client(&text) {
                    dispatch(&msg, &sessions, &clients);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    println!("[terminal-agent] frontend client disconnected");
    // The writer thread's channel sender still lives in `clients` until the
    // next broadcast finds it dead or full; the writer then closes the socket.
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
    let clients: Clients = hub::new_clients();

    for session in sessions.iter() {
        let session = session.clone();
        let clients = clients.clone();
        let generation = session.generation();
        thread::spawn(move || pump_session_output(session, clients, generation));
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

    let bind_addr = net::resolve_bind(env::var(net::BIND_ENV).ok().as_deref()).unwrap_or_else(|e| {
        eprintln!("[terminal-agent] {e}");
        std::process::exit(78);
    });
    if !bind_addr.ip().is_loopback() {
        eprintln!(
            "[terminal-agent] WARNING: listening on non-loopback address {bind_addr}. This is an \
             UNAUTHENTICATED SHELL reachable from the network. Use only on a trusted link."
        );
    }
    let listener = TcpListener::bind(bind_addr)
        .unwrap_or_else(|e| panic!("[terminal-agent] bind {bind_addr} failed: {e}"));
    println!("[terminal-agent] WebSocket server listening on ws://{bind_addr}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn config_parses_shipped_file() {
        let raw = include_str!("../terminal-agent-config.json");
        let cfg = parse_config(raw).unwrap();
        assert!(!cfg.sessions.is_empty());
    }

    #[test]
    fn config_rejects_bad_input() {
        assert!(parse_config("not json").is_err());
        assert!(parse_config(r#"{"sessions":[]}"#).is_err());
        assert!(parse_config(r#"{"sessions":[{"command":[]}]}"#).is_err());
        let ok = parse_config(r#"{"sessions":[{"name":"a","command":["sh"]}],"vermelho_host":"h"}"#).unwrap();
        assert_eq!(ok.vermelho_host.as_deref(), Some("h"));
        assert_eq!(ok.sessions[0].name.as_deref(), Some("a"));
    }

    #[test]
    fn handshake_origin_enforced() {
        use std::net::TcpListener;
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

    fn test_session(script: &str) -> Arc<Session> {
        Session::new(0, "t".into(), vec!["sh".into(), "-c".into(), script.into()]).unwrap()
    }

    fn test_clients() -> (Clients, std::sync::mpsc::Receiver<hub::Msg>) {
        let clients = hub::new_clients();
        let rx = hub::register(&clients);
        (clients, rx)
    }

    fn start_reader(s: &Arc<Session>, c: &Clients, generation: u64) {
        let (s, c) = (s.clone(), c.clone());
        thread::spawn(move || pump_session_output(s, c, generation));
    }

    fn pid_of(s: &Session) -> i32 {
        s.inner.lock().unwrap().child.as_ref().unwrap().id() as i32
    }

    /// True while `pid` still has a /proc entry that is not a zombie... or is one.
    fn is_zombie(pid: i32) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'),
            Err(_) => false,
        }
    }

    fn gone_or_alive_not_zombie_within(pid: i32, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn recv_matching(
        rx: &std::sync::mpsc::Receiver<hub::Msg>,
        secs: u64,
        pred: impl Fn(&Value) -> bool,
    ) -> Option<Value> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(m) = rx.recv_timeout(left) {
                let v: Value = serde_json::from_str(&m).unwrap();
                if pred(&v) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Regression: the old reader used to survive a manual respawn, so its
    /// EOF was counted as an involuntary exit of the new child.
    #[test]
    fn manual_respawn_is_not_an_involuntary_exit() {
        let (clients, rx) = test_clients();
        let s = test_session("exec cat");
        start_reader(&s, &clients, s.generation());
        thread::sleep(Duration::from_millis(200));
        let g = s.respawn().unwrap();
        assert_eq!(g, 2);
        start_reader(&s, &clients, g);
        thread::sleep(Duration::from_millis(700));
        let msgs: Vec<hub::Msg> = rx.try_iter().collect();
        assert!(msgs.iter().all(|m| !m.contains("exited")), "got {msgs:?}");
        assert_eq!(s.involuntary_exits.lock().unwrap().len(), 0);
    }

    #[test]
    fn stale_generation_reader_exits_immediately() {
        let (clients, _rx) = test_clients();
        let s = test_session("exec cat");
        let old = s.generation();
        s.respawn().unwrap();
        // Returns (instead of blocking on the live new fd) because it is stale.
        let t = Instant::now();
        pump_session_output(s.clone(), clients, old);
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn delayed_auto_respawn_yields_to_manual_respawn() {
        let s = test_session("exec cat");
        let old = s.generation();
        let manual = s.respawn().unwrap();
        assert_eq!(s.respawn_if(Some(old)), None);
        assert_eq!(s.generation(), manual);
        assert!(s.respawn_if(Some(manual)).is_some());
    }

    #[test]
    fn respawn_reaps_old_child() {
        let s = test_session("exec cat");
        let old_pid = pid_of(&s);
        s.respawn().unwrap();
        assert_ne!(pid_of(&s), old_pid);
        assert!(gone_or_alive_not_zombie_within(old_pid, 3), "old child {old_pid} not reaped");
        assert!(!is_zombie(old_pid));
    }

    #[test]
    fn exited_child_is_reaped_and_reported() {
        let (clients, rx) = test_clients();
        let s = test_session("exit 3");
        let pid = pid_of(&s);
        start_reader(&s, &clients, s.generation());
        let m = recv_matching(&rx, 5, |v| v["type"] == "exited").expect("exited message");
        assert_eq!(m["code"], 3);
        assert_eq!(m["auto_respawning"], true);
        assert!(gone_or_alive_not_zombie_within(pid, 3), "exited child {pid} left as zombie");
    }

    #[test]
    fn dispatch_input_round_trips_through_real_pty() {
        let (clients, rx) = test_clients();
        let s = test_session("exec cat");
        start_reader(&s, &clients, s.generation());
        let sessions = vec![s.clone()];
        let d = |raw: &str| dispatch(&decode_client(raw).unwrap(), &sessions, &clients);
        d(r#"{"type":"resize","session":0,"cols":80,"rows":24}"#);
        d(r#"{"type":"input","session":0,"data":"h\u00e9llo\n"}"#);
        let got = recv_matching(&rx, 5, |v| v["type"] == "output" && v["session"] == 0 && v["data"].as_str().unwrap().contains("h\u{e9}llo"));
        assert!(got.is_some());
        // Unknown session is ignored; unknown or malformed messages never decode.
        d(r#"{"type":"input","session":9,"data":"x"}"#);
        assert!(decode_client(r#"{"type":"bogus","session":0}"#).is_none());
        assert!(decode_client(r#"{"type":"input"}"#).is_none());
        let list = session_list_message(&sessions);
        assert_eq!(list, ServerMsg::SessionList { sessions: vec![SessionInfo { index: 0, name: "t".into() }] });
    }

    /// The SIGWINCH workaround (setsid + TIOCSCTTY): resizing the master must
    /// reach the child as SIGWINCH.
    #[test]
    fn resize_delivers_sigwinch_to_child() {
        let (clients, rx) = test_clients();
        let s = test_session("trap 'echo WINCH' WINCH; echo ready; while :; do sleep 0.05; done");
        start_reader(&s, &clients, s.generation());
        assert!(recv_matching(&rx, 5, |v| v["data"].as_str().is_some_and(|d| d.contains("ready"))).is_some());
        s.resize(100, 30);
        assert!(recv_matching(&rx, 5, |v| v["data"].as_str().is_some_and(|d| d.contains("WINCH"))).is_some());
        s.respawn();
    }

    // --- Wire characterization: current emitters vs committed fixtures -----

    fn fixture(name: &str) -> String {
        let path = format!("{}/../protocol/terminal/{name}.json", env!("CARGO_MANIFEST_DIR"));
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")).trim_end_matches('\n').to_string()
    }

    fn clock_msg(offset_ms: i64, status: ClockStatus, last_sync: Option<Instant>) -> String {
        let clock = Mutex::new(ClockState { offset_ms, status, last_sync });
        encode(&clock_sync_message(&clock))
    }

    #[test]
    fn current_emitters_match_golden_fixtures() {
        let s = |i: usize, n: &str| Session {
            index: i,
            name: n.to_string(),
            command: vec![],
            inner: Mutex::new(SessionInner { master_fd: None, child: None, generation: 1 }),
            involuntary_exits: Mutex::new(Vec::new()),
            respawn_lock: Mutex::new(()),
        };
        let sessions = vec![Arc::new(s(0, "shell-1")), Arc::new(s(2, "shell-3"))];
        assert_eq!(encode(&session_list_message(&sessions)), fixture("server_session_list"));
        assert_eq!(encode(&output_message(2, "h\u{e9}llo\r\n\u{1b}[0m")), fixture("server_output"));
        assert_eq!(encode(&exited_message(0, Some(3), true)), fixture("server_exited_auto_respawning"));
        assert_eq!(encode(&exited_message(1, None, false)), fixture("server_exited_needs_manual_respawn"));
        assert_eq!(clock_msg(-42, ClockStatus::Ok, Some(Instant::now())), fixture("server_clock_sync_ok"));
        let old = Instant::now() - Duration::from_secs(120);
        assert_eq!(clock_msg(1500, ClockStatus::Stale, Some(old)), fixture("server_clock_sync_stale"));
        assert_eq!(clock_msg(0, ClockStatus::Unsynced, None), fixture("server_clock_sync_unsynced"));
    }

    #[test]
    fn client_fixtures_are_what_dispatch_understands() {
        let v = |n: &str| serde_json::from_str::<Value>(&fixture(n)).unwrap();
        assert_eq!(v("client_input")["data"], "h\u{e9}llo\r");
        assert_eq!(v("client_resize")["cols"], 80);
        for (n, a) in [("interrupt", "interrupt"), ("clear", "clear"), ("respawn", "respawn")] {
            assert_eq!(v(&format!("client_control_{n}"))["action"], a);
        }
    }
}
