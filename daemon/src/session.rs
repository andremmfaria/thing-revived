//! One PTY-backed child process: lifecycle (spawn, respawn, reap), input,
//! resize and signals, plus the per-generation output reader thread.

use crate::hub::ClientHub;
use crate::protocol::ServerMsg;
use crate::pty::{self, PtyMaster, PtyReader};
use crate::respawn_policy::RespawnPolicy;
use crate::utf8::Utf8Carry;
use crate::utils::config::{PtyConfig, RespawnConfig};
use crate::utils::types::{Decision, GroupSignal, ReadOutcome, SessionId};
use std::process::Child;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// Mutable state behind one PTY-backed child process.
struct SessionInner {
    /// Shared so a write blocked on a wedged child never holds `inner`
    /// (which would also block respawn, the cure for a wedged child).
    master: Option<Arc<PtyMaster>>,
    child: Option<Child>,
    /// Bumped on every (re)spawn. A reader thread carries the generation it
    /// was started for and exits as soon as this no longer matches, so a
    /// manual respawn can never leave two readers on the new fd, nor have the
    /// old reader's EOF counted as an involuntary exit of the new child.
    generation: u64,
}

pub struct Session {
    pub index: SessionId,
    pub name: String,
    command: Vec<String>,
    pty: PtyConfig,
    inner: Mutex<SessionInner>,
    /// Policy over recent INVOLUNTARY exits only (detected via the reader's
    /// EOF/EIO path) - a manual "control"/"respawn" never touches it, so a
    /// user repeatedly respawning by hand can never trip the auto-backoff
    /// limiter meant for crash-looping commands. Design doc section 8.
    policy: Mutex<RespawnPolicy>,
    /// Serializes respawns (manual vs delayed auto) so two spawns can never
    /// race to install a child. Held across spawn, never with `inner`.
    respawn_lock: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Session {
    /// Spawns one configured session's PTY. Returns Err (never panics) on
    /// spawn failure, e.g. a configured command that isn't present on this
    /// firmware (ssh client not installed yet, etc.) - a bad entry in one
    /// session's config must not take down every other session or the
    /// WebSocket server itself.
    pub fn new(
        index: usize,
        name: String,
        command: Vec<String>,
        pty: &PtyConfig,
        respawn: &RespawnConfig,
    ) -> Result<Arc<Session>, String> {
        let (master, child) = pty::spawn(&command, initial_size(pty), &pty.term)
            .map_err(|e| format!("session {index} ({name}): spawn failed: {e}"))?;
        log::info!("session {index} ({name}): spawned pid={} cmd={:?}", child.id(), command);
        Ok(Arc::new(Session {
            index,
            name,
            command,
            pty: pty.clone(),
            inner: Mutex::new(SessionInner { master: Some(Arc::new(master)), child: Some(child), generation: 1 }),
            policy: Mutex::new(RespawnPolicy::new(respawn)),
            respawn_lock: Mutex::new(()),
        }))
    }

    /// Forwards keystrokes to the child. The data itself is never logged.
    pub fn write(&self, data: &str) {
        let master = lock(&self.inner).master.clone();
        if let Some(master) = master {
            if let Err(e) = master.write_all(data.as_bytes()) {
                log::warn!("session {}: write failed: {e}", self.index);
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let master = lock(&self.inner).master.clone();
        if let Some(master) = master {
            if let Err(e) = master.set_winsize(cols, rows) {
                log::warn!("session {}: resize failed: {e}", self.index);
            }
        }
    }

    /// SIGINT to the whole process group (see `pty::signal_group`).
    pub fn interrupt(&self) {
        let pid = match &lock(&self.inner).child {
            Some(c) => c.id(),
            None => return,
        };
        pty::signal_group(pid, GroupSignal::Interrupt);
    }

    /// Ctrl-L: same byte a human would type, not a true terminal reset -
    /// documented limitation, same as the Python original.
    pub fn clear(&self) {
        self.write("\x0c");
    }

    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    pub fn pid(&self) -> Option<u32> {
        lock(&self.inner).child.as_ref().map(|c| c.id())
    }

    /// Involuntary exits currently counted inside the respawn window.
    pub fn recent_involuntary_exits(&self) -> usize {
        lock(&self.policy).recent_exits()
    }

    /// Kills the old process group and reaps the old child, then spawns a
    /// fresh one. Returns the new generation to start a reader for, or None
    /// if the spawn failed. The generation is bumped before anything else so
    /// the old reader goes stale immediately.
    pub fn respawn(&self) -> Option<u64> {
        self.respawn_if(None)
    }

    /// Like `respawn`, but when `only_if_generation` is set, does nothing if
    /// the session was respawned (e.g. manually) since that generation. The
    /// delayed auto-respawn uses this so it cannot clobber a newer child.
    pub fn respawn_if(&self, only_if_generation: Option<u64>) -> Option<u64> {
        let _serialize = lock(&self.respawn_lock);
        let (old_child, old_master) = {
            let mut inner = lock(&self.inner);
            if let Some(g) = only_if_generation {
                if inner.generation != g {
                    return None;
                }
            }
            inner.generation += 1;
            (inner.child.take(), inner.master.take())
        };
        if let Some(mut child) = old_child {
            // The child is a session leader (setsid), so its pid is the pgid.
            pty::signal_group(child.id(), GroupSignal::Kill);
            // SIGKILL makes this return promptly; a thread keeps an
            // uninterruptible child from ever stalling the caller.
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        drop(old_master);
        match pty::spawn(&self.command, initial_size(&self.pty), &self.pty.term) {
            Ok((master, child)) => {
                log::info!("session {} ({}): respawned pid={}", self.index, self.name, child.id());
                let mut inner = lock(&self.inner);
                inner.master = Some(Arc::new(master));
                inner.child = Some(child);
                Some(inner.generation)
            }
            Err(e) => {
                log::error!("session {}: respawn failed: {e}", self.index);
                None
            }
        }
    }

    /// A private reader for `generation`, or None if that generation is
    /// stale. Owning a dup means the fd number cannot be closed and reused
    /// by another session while the reader is mid-read.
    fn reader(&self, generation: u64) -> Option<PtyReader> {
        let inner = lock(&self.inner);
        if inner.generation != generation {
            return None;
        }
        inner.master.as_ref()?.reader().ok()
    }

    /// Exit code of the child of `generation`, reaping it. Outer None means
    /// the generation is stale (respawned meanwhile); inner None means the
    /// child could not be reaped yet. try_wait is polled briefly because EOF
    /// on the master can arrive a moment before the child is reapable.
    fn reap_exit(&self, generation: u64) -> Option<Option<i32>> {
        for attempt in 0..20 {
            {
                let mut inner = lock(&self.inner);
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
}

fn initial_size(cfg: &PtyConfig) -> Option<(u16, u16)> {
    (cfg.initial_cols != 0 || cfg.initial_rows != 0).then_some((cfg.initial_cols, cfg.initial_rows))
}

/// Starts the output reader thread for `generation` of `session`.
pub fn spawn_reader(session: &Arc<Session>, clients: &ClientHub, generation: u64) {
    let (session, clients) = (session.clone(), clients.clone());
    thread::spawn(move || pump_output(session, clients, generation));
}

/// Continuously reads one session's PTY and broadcasts the output. On the
/// child's exit (EOF/EIO), applies the bounded auto-respawn policy from
/// design doc section 8: up to `respawn.max` involuntary exits within the
/// window get an automatic retry (with increasing delay) and a fresh reader
/// thread; the next one in the same window gives up and asks for a manual
/// respawn via `needs_manual_respawn`.
pub fn pump_output(session: Arc<Session>, clients: ClientHub, generation: u64) {
    let reader = match session.reader(generation) {
        Some(r) => r,
        None => return,
    };
    let mut buf = vec![0u8; session.pty.read_buffer_size];
    let mut carry = Utf8Carry::new();
    loop {
        // Wake periodically so a reader whose generation went stale (manual
        // respawn) exits even if its old PTY never signals EOF.
        match reader.read_timeout(&mut buf, session.pty.reader_poll_ms) {
            ReadOutcome::Idle => {
                if session.generation() != generation {
                    return;
                }
            }
            ReadOutcome::Interrupted => {}
            ReadOutcome::Data(n) => {
                if session.generation() != generation {
                    return;
                }
                let chunk = carry.push(&buf[..n]);
                if chunk.is_empty() {
                    continue;
                }
                log::trace!("session {}: {n} bytes from pty", session.index);
                clients.broadcast(&ServerMsg::Output { session: session.index, data: chunk });
            }
            ReadOutcome::Eof => {
                // The child exited. Stale generations (this was a manual
                // respawn) exit silently and are not counted.
                if session.generation() != generation {
                    return;
                }
                let tail = carry.finish();
                if !tail.is_empty() {
                    clients.broadcast(&ServerMsg::Output { session: session.index, data: tail });
                }
                handle_involuntary_exit(&session, &clients, generation);
                return;
            }
            ReadOutcome::Failed(err) => {
                log::error!("session {}: read error: {err}", session.index);
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Shared exit-handling path for both EOF and EIO ("slave closed") cases:
/// record the involuntary exit, broadcast the (possibly augmented) `exited`
/// message, and either queue a bounded auto-respawn or give up and flag the
/// session for manual recovery.
fn handle_involuntary_exit(session: &Arc<Session>, clients: &ClientHub, generation: u64) {
    let code = match session.reap_exit(generation) {
        Some(code) => code,
        None => return, // respawned while we were waiting: not an involuntary exit
    };
    let decision = lock(&session.policy).on_exit(Instant::now());
    match decision {
        Decision::Retry { attempt, delay } => {
            let window_s = lock(&session.policy).window().as_secs();
            log::warn!(
                "session {}: involuntary exit #{attempt} within {window_s}s window, auto-respawning in {}s",
                session.index,
                delay.as_secs()
            );
            clients.broadcast(
                &ServerMsg::Exited {
                    session: session.index,
                    code,
                    auto_respawning: true,
                    needs_manual_respawn: false,
                },
            );
            let session = session.clone();
            let clients = clients.clone();
            thread::spawn(move || {
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                // A fresh child needs a fresh reader thread - the old reader
                // returned on the old child's EOF/EIO. Skipped if a manual
                // respawn already replaced the child during the delay.
                if let Some(g) = session.respawn_if(Some(generation)) {
                    spawn_reader(&session, &clients, g);
                }
            });
        }
        Decision::GiveUp { attempt } => {
            let window_s = lock(&session.policy).window().as_secs();
            log::error!(
                "session {}: {attempt} involuntary exits within {window_s}s - crash-looping, not transient; \
                 giving up on auto-respawn, needs manual trigger",
                session.index
            );
            clients.broadcast(
                &ServerMsg::Exited {
                    session: session.index,
                    code,
                    auto_respawning: false,
                    needs_manual_respawn: true,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::types::Msg;
    use crate::protocol::ServerMsg;
    use std::fs;
    use std::sync::mpsc::Receiver;

    fn test_session(script: &str) -> Arc<Session> {
        Session::new(
            0,
            "t".into(),
            vec!["sh".into(), "-c".into(), script.into()],
            &PtyConfig::default(),
            &RespawnConfig::default(),
        )
        .unwrap()
    }

    fn test_clients() -> (ClientHub, Receiver<Msg>) {
        let clients = ClientHub::new(256);
        let rx = clients.register();
        (clients, rx)
    }

    fn is_zombie(pid: i32) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'),
            Err(_) => false,
        }
    }

    fn gone_within(pid: i32, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn recv_matching(rx: &Receiver<Msg>, secs: u64, pred: impl Fn(&ServerMsg) -> bool) -> Option<ServerMsg> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(m) = rx.recv_timeout(left) {
                let v: ServerMsg = serde_json::from_str(&m).unwrap();
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
        spawn_reader(&s, &clients, s.generation());
        thread::sleep(Duration::from_millis(200));
        let g = s.respawn().unwrap();
        assert_eq!(g, 2);
        spawn_reader(&s, &clients, g);
        thread::sleep(Duration::from_millis(700));
        let msgs: Vec<Msg> = rx.try_iter().collect();
        assert!(msgs.iter().all(|m| !m.contains("exited")), "got {msgs:?}");
        assert_eq!(s.recent_involuntary_exits(), 0);
    }

    #[test]
    fn stale_generation_reader_exits_immediately() {
        let (clients, _rx) = test_clients();
        let s = test_session("exec cat");
        let old = s.generation();
        s.respawn().unwrap();
        // Returns (instead of blocking on the live new fd) because it is stale.
        let t = Instant::now();
        pump_output(s.clone(), clients, old);
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
        let old_pid = s.pid().unwrap() as i32;
        s.respawn().unwrap();
        assert_ne!(s.pid().unwrap() as i32, old_pid);
        assert!(gone_within(old_pid, 3), "old child {old_pid} not reaped");
        assert!(!is_zombie(old_pid));
    }

    #[test]
    fn exited_child_is_reaped_and_reported() {
        let (clients, rx) = test_clients();
        let s = test_session("exit 3");
        let pid = s.pid().unwrap() as i32;
        spawn_reader(&s, &clients, s.generation());
        let m = recv_matching(&rx, 5, |v| matches!(v, ServerMsg::Exited { .. })).expect("exited message");
        assert_eq!(
            m,
            ServerMsg::Exited { session: 0, code: Some(3), auto_respawning: true, needs_manual_respawn: false }
        );
        assert!(gone_within(pid, 3), "exited child {pid} left as zombie");
    }

    #[test]
    fn initial_size_is_applied_when_configured() {
        let pty = PtyConfig { initial_cols: 100, initial_rows: 30, ..PtyConfig::default() };
        let s = Session::new(
            0,
            "t".into(),
            vec!["sh".into(), "-c".into(), "stty size; exec cat".into()],
            &pty,
            &RespawnConfig::default(),
        )
        .unwrap();
        let (clients, rx) = test_clients();
        spawn_reader(&s, &clients, s.generation());
        let got = recv_matching(&rx, 5, |v| matches!(v, ServerMsg::Output { data, .. } if data.contains("30 100")));
        assert!(got.is_some());
    }
}
