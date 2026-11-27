//! All raw libc / unsafe code of the agent lives here: opening a PTY pair,
//! spawning the child on it, reading, writing, resizing and signalling.
//!
//! `PtyMaster` owns the master fd and closes it on drop. Readers get their
//! own `dup` (`PtyReader`), so an fd number can never be closed and reused by
//! another session while a reader is mid-read.

use crate::utils::types::{GroupSignal, ReadOutcome};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// The master side of a PTY. Dropping it closes the fd.
#[derive(Debug)]
pub struct PtyMaster {
    fd: OwnedFd,
}

/// A private duplicate of the master fd for one reader thread.
#[derive(Debug)]
pub struct PtyReader {
    fd: OwnedFd,
}

/// Sends `signal` to the process group of `pid`. The child called `setsid()`,
/// so its process group id equals its own pid; a negative pid in `kill(2)`
/// targets the whole group, same effect as Python's
/// `os.killpg(os.getpgid(pid), SIGINT)`.
pub fn signal_group(pid: u32, signal: GroupSignal) {
    let sig = match signal {
        GroupSignal::Interrupt => libc::SIGINT,
        GroupSignal::Kill => libc::SIGKILL,
    };
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

impl PtyMaster {
    /// Writes all of `bytes` to the child (partial writes are completed).
    pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let rc = unsafe { libc::write(self.fd.as_raw_fd(), rest.as_ptr() as *const _, rest.len()) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            rest = &rest[rc as usize..];
        }
        Ok(())
    }

    /// `TIOCSWINSZ` on the master. The kernel raises SIGWINCH for the
    /// terminal's foreground group only if the child has a controlling
    /// terminal (see `spawn`).
    pub fn set_winsize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        let rc = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// A reader on its own duplicate of the master fd.
    pub fn reader(&self) -> io::Result<PtyReader> {
        let d = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if d < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(PtyReader { fd: unsafe { OwnedFd::from_raw_fd(d) } })
    }
}

impl PtyReader {
    /// Waits up to `timeout_ms` for output, then reads at most `buf.len()` bytes.
    pub fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> ReadOutcome {
        let raw = self.fd.as_raw_fd();
        let mut pfd = libc::pollfd { fd: raw, events: libc::POLLIN, revents: 0 };
        let pr = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if pr == 0 {
            return ReadOutcome::Idle;
        }
        if pr < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EINTR) {
                ReadOutcome::Interrupted
            } else {
                ReadOutcome::Failed(err)
            };
        }
        let n = unsafe { libc::read(raw, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            return ReadOutcome::Data(n as usize);
        }
        let err = io::Error::last_os_error();
        if n == 0 || err.raw_os_error() == Some(libc::EIO) {
            return ReadOutcome::Eof;
        }
        if err.raw_os_error() == Some(libc::EINTR) {
            return ReadOutcome::Interrupted;
        }
        ReadOutcome::Failed(err)
    }
}

/// Opens a PTY pair and spawns `command` on the slave side, in its own
/// session/process group (`setsid`), matching the Python original's
/// `preexec_fn=os.setsid` - needed so "interrupt" can signal the whole
/// group, not just the immediate pid. `size` is the initial window size
/// (`None` leaves the kernel default of 0x0 until the first resize).
pub fn spawn(command: &[String], size: Option<(u16, u16)>, term: &str) -> io::Result<(PtyMaster, Child)> {
    spawn_with_inherited_term(command, size, term, std::env::var("TERM").ok().as_deref())
}

/// `term` is the configured TERM; it is applied (with `COLORTERM=truecolor`)
/// unless the agent's own TERM (`inherited`) is something other than empty
/// or `dumb`. Neither the agent nor its unit sets TERM, so bash falls back
/// to `dumb` before /etc/profile runs and `clear`/htop break without this.
fn spawn_with_inherited_term(
    command: &[String],
    size: Option<(u16, u16)>,
    term: &str,
    inherited: Option<&str>,
) -> io::Result<(PtyMaster, Child)> {
    // Serialize the openpty..spawn window process-wide. Without this a
    // concurrent spawn (another session, or a respawn) can fork while this
    // call's slave fds are open and non-CLOEXEC, inherit them, and keep the
    // slave open so this session never sees EOF when its child exits.
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());
    let _spawn_guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut winsize = size.map(|(cols, rows)| libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    });
    let winsize_ptr = match winsize.as_mut() {
        Some(ws) => ws as *mut libc::winsize,
        None => std::ptr::null_mut(),
    };
    // SAFETY: `openpty` is a glibc/BSD extension; we pass null for
    // name/termios, and null winsize unless an initial size was configured,
    // matching Python's `pty.openpty()` defaults.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            winsize_ptr,
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
    // From here the master closes itself on every early return.
    let master = PtyMaster { fd: unsafe { OwnedFd::from_raw_fd(master) } };

    let prog = &command[0];
    let args = &command[1..];
    let mut cmd = Command::new(prog);
    cmd.args(args);
    if matches!(inherited, None | Some("") | Some("dumb")) {
        cmd.env("TERM", term).env("COLORTERM", "truecolor");
    }

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

    let child = cmd.spawn()?;

    Ok((master, child))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn echo_term(inherited: Option<&str>) -> String {
        let cmd = vec!["sh".to_string(), "-c".to_string(), "echo \"$TERM $COLORTERM\"".to_string()];
        let (master, mut child) = spawn_with_inherited_term(&cmd, None, "xterm-256color", inherited).unwrap();
        let reader = master.reader().unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match reader.read_timeout(&mut buf, 200) {
                ReadOutcome::Data(n) => out.extend_from_slice(&buf[..n]),
                ReadOutcome::Eof => break,
                _ => {}
            }
        }
        child.wait().unwrap();
        String::from_utf8_lossy(&out).trim().to_string()
    }

    #[test]
    fn unset_empty_or_dumb_term_gets_the_configured_term() {
        for inherited in [None, Some(""), Some("dumb")] {
            assert_eq!(echo_term(inherited), "xterm-256color truecolor", "inherited {inherited:?}");
        }
    }

    #[test]
    fn a_real_inherited_term_is_left_alone() {
        // The child inherits this test process's own TERM, so only assert
        // that the configured value was not forced onto it.
        let out = echo_term(Some("screen"));
        assert_eq!(out, format!("{} {}", std::env::var("TERM").unwrap_or_default(), std::env::var("COLORTERM").unwrap_or_default()).trim());
    }
}
