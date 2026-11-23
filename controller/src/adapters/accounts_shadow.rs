//! Account adapters on the real system.
//!
//! - `ShadowAccounts` (`AccountsPort`): spawns the shadow-utils binaries by
//!   absolute path with an argument vector, a cleared environment and the
//!   password on stdin only (plan section 7). No shell, no string
//!   concatenation of commands.
//! - `JsonStateFile` (`AccountStateStore`): `state.json`, atomic and 0600.
//! - `EtcFiles` (`AccountFiles`): the `/etc` overlay reconcile and the key
//!   store housekeeping.
//!
//! Behaviour taken from docs/research/user-management-spikes.md: `usermod
//! --unlock` on a `!`-only hash exits 0 and changes nothing, so the hash is
//! checked here first; `loginctl terminate-user` exits 1 for an account with
//! no logind session, which is not an error; processes are found by real uid
//! in `/proc` and ended with `/usr/bin/kill -KILL`.

use crate::app::error::AppError;
use crate::domain::accounts::password::Secret;
use crate::domain::accounts::state::{hash_is_usable, is_safe_account_name, AccountFile, AccountState};
use crate::ports::{AccountAdmin, AccountFiles, AccountStateStore, AccountsError, AccountsPort};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CHPASSWD: &str = "/usr/sbin/chpasswd";
pub const USERMOD: &str = "/usr/sbin/usermod";
pub const LOGINCTL: &str = "/usr/bin/loginctl";
pub const KILL: &str = "/usr/bin/kill";
pub const USERADD: &str = "/usr/sbin/useradd";
pub const USERDEL: &str = "/usr/sbin/userdel";
pub const GROUPADD: &str = "/usr/sbin/groupadd";
pub const GROUPDEL: &str = "/usr/sbin/groupdel";
pub const GPASSWD: &str = "/usr/bin/gpasswd";
pub const SYSTEMCTL: &str = "/usr/bin/systemctl";
/// The terminal agent unit that a `kiosk` lock stops.
pub const TERMINAL_UNIT: &str = "thing-revived-terminal-agent.service";
/// Login shell of every account the menu creates.
pub const USER_SHELL: &str = "/usr/bin/bash";
/// Every tool runs with exactly this environment.
pub const TOOL_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
/// sha512 crypt rounds, measured on the device (spikes report, item h).
pub const DEFAULT_SHA_ROUNDS: u32 = 65536;

// ---------------------------------------------------------------------------
// Command execution
// ---------------------------------------------------------------------------

/// One command: absolute program, argument vector, optional stdin bytes.
/// `Debug` shows the program and arguments and only the length of stdin,
/// because stdin carries the password.
pub struct CommandSpec {
    pub program: &'static str,
    pub args: Vec<OsString>,
    pub stdin: Vec<u8>,
}

impl CommandSpec {
    pub fn new(program: &'static str, args: &[&str]) -> CommandSpec {
        CommandSpec { program, args: args.iter().map(OsString::from).collect(), stdin: Vec::new() }
    }
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec").field("program", &self.program).field("args", &self.args).field("stdin_len", &self.stdin.len()).finish()
    }
}

impl Drop for CommandSpec {
    fn drop(&mut self) {
        self.stdin.iter_mut().for_each(|b| *b = 0);
        std::hint::black_box(&self.stdin);
    }
}

/// Runs a command and returns its exit status. Output is discarded: it can
/// echo names and the tools print nothing a caller needs beyond the status.
pub trait Runner: Send + Sync {
    fn run(&self, spec: &CommandSpec) -> Result<i32, AppError>;
    /// Like `run`, but also returns what the tool wrote to stdout (capped at
    /// 4 KiB). Only for tools whose output is data (`ssh-keygen -l`), never
    /// for one that handles a password.
    fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError>;
}

pub struct SystemRunner {
    timeout: Duration,
}

impl SystemRunner {
    pub fn new() -> SystemRunner {
        SystemRunner { timeout: TOOL_TIMEOUT }
    }

    /// `[users] tool_timeout_secs`.
    pub fn with_timeout(timeout: Duration) -> SystemRunner {
        SystemRunner { timeout }
    }
}

impl Default for SystemRunner {
    fn default() -> Self {
        SystemRunner::new()
    }
}

impl Runner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32, AppError> {
        self.exec(spec, false).map(|(code, _)| code)
    }

    fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError> {
        self.exec(spec, true)
    }
}

impl SystemRunner {
    fn exec(&self, spec: &CommandSpec, capture: bool) -> Result<(i32, Vec<u8>), AppError> {
        let mut cmd = Command::new(spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .env("PATH", TOOL_PATH)
            .env("LANG", "C")
            .stdout(if capture { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::null())
            .stdin(if spec.stdin.is_empty() { Stdio::null() } else { Stdio::piped() });
        let mut child = cmd.spawn().map_err(|e| AppError::failed(format!("spawn {}: {e}", spec.program)))?;
        if let Some(mut stdin) = child.stdin.take() {
            // A tool that exits early closes the pipe; its status says why.
            if let Err(e) = stdin.write_all(&spec.stdin) {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::failed(format!("write to {}: {e}", spec.program)));
                }
            }
        }
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code().ok_or_else(|| AppError::failed(format!("{} was killed by a signal", spec.program)))?;
                    let mut out = Vec::new();
                    if let Some(stdout) = child.stdout.take() {
                        // The child has exited, so this only drains the pipe.
                        let _ = std::io::Read::read_to_end(&mut std::io::Read::take(stdout, 4096), &mut out);
                    }
                    return Ok((code, out));
                }
                Ok(None) if start.elapsed() >= self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::failed(format!("{} timed out", spec.program)));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(AppError::failed(format!("wait {}: {e}", spec.program))),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AccountsPort
// ---------------------------------------------------------------------------

pub struct ShadowAccounts<R: Runner> {
    runner: R,
    shadow_path: PathBuf,
    passwd_path: PathBuf,
    proc_dir: PathBuf,
    sha_rounds: u32,
    settle: Duration,
    home_base: PathBuf,
    /// Flag file the terminal unit's `ConditionPathExists=!` watches.
    kiosk_flag: PathBuf,
}

impl ShadowAccounts<SystemRunner> {
    pub fn new(sha_rounds: u32) -> ShadowAccounts<SystemRunner> {
        ShadowAccounts::with_runner(SystemRunner::new(), sha_rounds)
    }
}

impl<R: Runner> ShadowAccounts<R> {
    pub fn with_runner(runner: R, sha_rounds: u32) -> ShadowAccounts<R> {
        ShadowAccounts {
            runner,
            shadow_path: PathBuf::from("/etc/shadow"),
            passwd_path: PathBuf::from("/etc/passwd"),
            proc_dir: PathBuf::from("/proc"),
            sha_rounds,
            settle: Duration::from_millis(50),
            home_base: PathBuf::from("/var/home"),
            kiosk_flag: PathBuf::from("/var/lib/thing-revived/accounts/kiosk.locked"),
        }
    }

    /// Reads account data from other files (tests).
    pub fn with_paths(mut self, passwd: impl Into<PathBuf>, shadow: impl Into<PathBuf>, proc_dir: impl Into<PathBuf>) -> Self {
        self.passwd_path = passwd.into();
        self.shadow_path = shadow.into();
        self.proc_dir = proc_dir.into();
        self.settle = Duration::ZERO;
        self
    }

    /// `[users] home_base` and the account state directory (where the
    /// `kiosk.locked` flag lives).
    pub fn with_dirs(mut self, home_base: impl Into<PathBuf>, state_dir: impl AsRef<Path>) -> Self {
        self.home_base = home_base.into();
        self.kiosk_flag = state_dir.as_ref().join("kiosk.locked");
        self
    }

    fn check_name(name: &str) -> Result<(), AccountsError> {
        if is_safe_account_name(name) {
            Ok(())
        } else {
            Err(AccountsError::InvalidName)
        }
    }

    fn run_ok(&self, spec: CommandSpec) -> Result<(), AccountsError> {
        match self.runner.run(&spec).map_err(AccountsError::Tool)? {
            0 => Ok(()),
            code => Err(AccountsError::Tool(AppError::failed(format!("{} exited with status {code}", spec.program)))),
        }
    }

    fn shadow_hash(&self, user: &str) -> Result<String, AccountsError> {
        let text = fs::read_to_string(&self.shadow_path).map_err(|e| AccountsError::Tool(AppError::failed(format!("read {}: {e}", self.shadow_path.display()))))?;
        text.lines()
            .filter_map(|l| {
                let mut f = l.split(':');
                Some((f.next()?, f.next()?))
            })
            .find(|(n, _)| *n == user)
            .map(|(_, h)| h.to_string())
            .ok_or(AccountsError::NotFound)
    }

    fn uid_of(&self, user: &str) -> Result<u32, AccountsError> {
        let text = fs::read_to_string(&self.passwd_path).map_err(|e| AccountsError::Tool(AppError::failed(format!("read {}: {e}", self.passwd_path.display()))))?;
        text.lines()
            .filter_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                let uid = f.nth(1)?.parse::<u32>().ok()?;
                Some((name, uid))
            })
            .find(|(n, _)| *n == user)
            .map(|(_, uid)| uid)
            .ok_or(AccountsError::NotFound)
    }

    fn pids_of(&self, uid: u32) -> Vec<u32> {
        let Ok(entries) = fs::read_dir(&self.proc_dir) else { return Vec::new() };
        let mut pids: Vec<u32> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                fs::read_to_string(self.proc_dir.join(pid.to_string()).join("status")).ok().is_some_and(|s| status_matches(&s, uid))
            })
            .collect();
        pids.sort_unstable();
        pids
    }
}

/// True for a live (not zombie) process whose real uid is `uid`. The real uid
/// is what catches a setuid child (`sudo`) whose effective uid is root.
fn status_matches(status: &str, uid: u32) -> bool {
    let real = status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u32>().ok());
    let zombie = status.lines().find_map(|l| l.strip_prefix("State:")).is_some_and(|s| s.trim_start().starts_with('Z'));
    real == Some(uid) && !zombie
}

impl<R: Runner> AccountsPort for ShadowAccounts<R> {
    fn set_password(&self, user: &str, password: &Secret) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        let pw = password.expose();
        if pw.is_empty() || pw.contains(['\n', '\r', '\0']) {
            return Err(AccountsError::InvalidPassword);
        }
        let rounds = self.sha_rounds.to_string();
        let mut spec = CommandSpec::new(CHPASSWD, &["--crypt-method", "SHA512", "--sha-rounds", &rounds]);
        spec.stdin = format!("{user}:{pw}\n").into_bytes();
        self.run_ok(spec)
    }

    fn clear_password(&self, user: &str) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        // `!` is not a secret, so it may travel in argv; the hash that was
        // there is replaced, not kept behind a lock prefix.
        self.run_ok(CommandSpec::new(USERMOD, &["--password", "!", user]))
    }

    fn set_locked(&self, user: &str, locked: bool) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        if locked {
            return self.run_ok(CommandSpec::new(USERMOD, &["--lock", "--expiredate", "1", user]));
        }
        // usermod exits 0 when it refuses to unlock a hash of exactly `!`.
        if !hash_is_usable(&self.shadow_hash(user)?) {
            return Err(AccountsError::NoPassword);
        }
        self.run_ok(CommandSpec::new(USERMOD, &["--unlock", "--expiredate", "", user]))
    }

    fn end_sessions(&self, user: &str) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        let uid = self.uid_of(user)?;
        if uid == 0 {
            return Err(AccountsError::InvalidName);
        }
        // Exit status 1 means "not logged in or lingering"; the /proc scan
        // below is what really ends the account's processes, so no status of
        // this call is an error.
        if let Err(e) = self.runner.run(&CommandSpec::new(LOGINCTL, &["terminate-user", user])) {
            warn!("accounts: loginctl terminate-user for {user} did not run: {e}");
        }
        let mut pids = self.pids_of(uid);
        if pids.is_empty() {
            return Ok(());
        }
        let mut args: Vec<&str> = vec!["-KILL"];
        let owned: Vec<String> = pids.iter().map(u32::to_string).collect();
        args.extend(owned.iter().map(String::as_str));
        // `kill` exits non-zero if one pid vanished first; the rescan decides.
        let _ = self.runner.run(&CommandSpec::new(KILL, &args));
        for _ in 0..10 {
            pids = self.pids_of(uid);
            if pids.is_empty() {
                return Ok(());
            }
            std::thread::sleep(self.settle);
        }
        warn!("accounts: {} process(es) of {user} survived the kill", pids.len());
        Err(AccountsError::SessionsRemain)
    }
}

impl<R: Runner> AccountAdmin for ShadowAccounts<R> {
    fn add_user(&self, name: &str) -> Result<(), AccountsError> {
        Self::check_name(name)?;
        let home = self.home_base.join(name);
        // No `--password`: the account is created locked until the service
        // sets one, and rolled back if that fails.
        let mut spec = CommandSpec::new(USERADD, &["--create-home", "--home-dir"]);
        spec.args.push(home.into_os_string());
        spec.args.extend(["--shell", USER_SHELL, "--user-group", "--comment", "", name].map(OsString::from));
        self.run_ok(spec)
    }

    fn remove_user(&self, name: &str, keep_home: bool) -> Result<(), AccountsError> {
        Self::check_name(name)?;
        let mut args: Vec<&str> = Vec::new();
        if !keep_home {
            args.push("--remove");
        }
        args.push(name);
        let spec = CommandSpec::new(USERDEL, &args);
        match self.runner.run(&spec).map_err(AccountsError::Tool)? {
            0 => Ok(()),
            // userdel: 6 = no such user, 8 = the user is logged in.
            6 => Err(AccountsError::NotFound),
            8 => Err(AccountsError::Busy),
            code => Err(AccountsError::Tool(AppError::failed(format!("{USERDEL} exited with status {code}")))),
        }
    }

    fn add_group(&self, name: &str) -> Result<(), AccountsError> {
        Self::check_name(name)?;
        self.run_ok(CommandSpec::new(GROUPADD, &[name]))
    }

    fn remove_group(&self, name: &str) -> Result<(), AccountsError> {
        Self::check_name(name)?;
        self.run_ok(CommandSpec::new(GROUPDEL, &[name]))
    }

    fn add_member(&self, user: &str, group: &str) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        Self::check_name(group)?;
        self.run_ok(CommandSpec::new(GPASSWD, &["--add", user, group]))
    }

    fn remove_member(&self, user: &str, group: &str) -> Result<(), AccountsError> {
        Self::check_name(user)?;
        Self::check_name(group)?;
        self.run_ok(CommandSpec::new(GPASSWD, &["--delete", user, group]))
    }

    fn set_kiosk_terminal(&self, enabled: bool) -> Result<(), AccountsError> {
        // The flag first on a lock, so a restart racing the stop is refused
        // by the unit's condition; removed first on an unlock for the same
        // reason in reverse.
        if enabled {
            match fs::remove_file(&self.kiosk_flag) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(AccountsError::Tool(AppError::failed(format!("remove {}: {e}", self.kiosk_flag.display())))),
            }
            return self.run_ok(CommandSpec::new(SYSTEMCTL, &["start", TERMINAL_UNIT]));
        }
        atomic_write(&self.kiosk_flag, b"", 0o600, None)
            .map_err(|e| AccountsError::Tool(AppError::failed(format!("write {}: {e}", self.kiosk_flag.display()))))?;
        self.run_ok(CommandSpec::new(SYSTEMCTL, &["stop", TERMINAL_UNIT]))
    }
}

// ---------------------------------------------------------------------------
// Atomic file replace
// ---------------------------------------------------------------------------

/// Temp file in the same directory, fsync, rename, fsync the directory. A
/// power cut leaves the old file or the new one. `mode`, `owner` apply to the
/// new file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".thing-revived.tmp");
    let tmp = parent.join(tmp_name);
    let result = (|| {
        let _ = fs::remove_file(&tmp);
        let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::chown(&tmp, Some(uid), Some(gid))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------------------
// state.json
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct StateFile {
    epoch: String,
    kiosk_password_set: bool,
    acknowledged: bool,
}

pub struct JsonStateFile {
    path: PathBuf,
}

impl JsonStateFile {
    pub fn new(state_dir: impl AsRef<Path>) -> JsonStateFile {
        JsonStateFile { path: state_dir.as_ref().join("state.json") }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AccountStateStore for JsonStateFile {
    fn load(&self) -> Result<Option<AccountState>, AppError> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AppError::failed(format!("read {}: {e}", self.path.display()))),
        };
        match serde_json::from_str::<StateFile>(&raw) {
            Ok(f) => Ok(Some(AccountState { epoch: f.epoch, kiosk_password_set: f.kiosk_password_set, acknowledged: f.acknowledged })),
            Err(e) => {
                // Not "missing": a missing file next to a usable hash means a
                // hand-set password (Adopt), but a corrupt one may be ours, so
                // it is unacknowledged. The epoch is a fixed placeholder; it is
                // only stored, and the next save rewrites the file.
                warn!("accounts: {} does not parse ({e}), treating it as unacknowledged", self.path.display());
                Ok(Some(AccountState { epoch: "0".repeat(32), kiosk_password_set: true, acknowledged: false }))
            }
        }
    }

    fn save(&self, state: &AccountState) -> Result<(), AppError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::os::unix::fs::DirBuilderExt::mode(fs::DirBuilder::new().recursive(true), 0o700)
            .create(parent)
            .map_err(|e| AppError::failed(format!("create {}: {e}", parent.display())))?;
        let json = serde_json::to_string_pretty(&StateFile { epoch: state.epoch.clone(), kiosk_password_set: state.kiosk_password_set, acknowledged: state.acknowledged })
            .map_err(AppError::failed)?;
        atomic_write(&self.path, json.as_bytes(), 0o600, None).map_err(|e| AppError::failed(format!("write {}: {e}", self.path.display())))
    }
}

// ---------------------------------------------------------------------------
// Reconcile files
// ---------------------------------------------------------------------------

/// Locations the reconcile works on. Defaults follow the plan's `[users]`
/// section and the mount unit's bind of the lower layer.
#[derive(Debug, Clone)]
pub struct EtcPaths {
    pub etc_dir: PathBuf,
    pub lower_dir: PathBuf,
    pub upper_dir: PathBuf,
    pub keys_dir: PathBuf,
    pub mountinfo: PathBuf,
}

impl Default for EtcPaths {
    fn default() -> Self {
        EtcPaths {
            etc_dir: PathBuf::from("/etc"),
            lower_dir: PathBuf::from("/run/thing-revived/etc-lower"),
            upper_dir: PathBuf::from("/var/lib/thing-revived/etc/upper"),
            keys_dir: PathBuf::from("/var/lib/thing-revived-ssh/authorized_keys"),
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
        }
    }
}

pub struct EtcFiles {
    paths: EtcPaths,
}

impl EtcFiles {
    pub fn new(paths: EtcPaths) -> EtcFiles {
        EtcFiles { paths }
    }
}

/// True when `mountinfo` lists an overlay filesystem mounted at `mount_point`.
/// The lower bind alone is not proof: a failed mount script can leave it
/// behind, and reconciling against a plain read-only `/etc` would delete the
/// key files of managed users.
fn overlay_mounted_on(mountinfo: &str, mount_point: &str) -> bool {
    mountinfo.lines().any(|l| {
        let (before, after) = match l.split_once(" - ") {
            Some(p) => p,
            None => return false,
        };
        let point = before.split(' ').nth(4);
        let fstype = after.split(' ').next();
        point == Some(mount_point) && fstype == Some("overlay")
    })
}

fn read_text(path: &Path) -> Result<String, AppError> {
    fs::read_to_string(path).map_err(|e| AppError::failed(format!("read {}: {e}", path.display())))
}

/// A file the image does not ship (for example `gshadow`) reads as empty.
fn read_or_empty(path: &Path) -> Result<String, AppError> {
    match fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(AppError::failed(format!("read {}: {e}", path.display()))),
    }
}

fn sorted_names(dir: &Path) -> Result<Vec<String>, AppError> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AppError::failed(format!("list {}: {e}", dir.display()))),
    };
    let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    Ok(names)
}

impl AccountFiles for EtcFiles {
    fn storage_available(&self) -> bool {
        let mounted = fs::read_to_string(&self.paths.mountinfo).is_ok_and(|m| overlay_mounted_on(&m, &self.paths.etc_dir.to_string_lossy()));
        mounted && self.paths.lower_dir.join("passwd").is_file()
    }

    fn read_lower(&self, file: AccountFile) -> Result<String, AppError> {
        read_or_empty(&self.paths.lower_dir.join(file.file_name()))
    }

    fn read_current(&self, file: AccountFile) -> Result<String, AppError> {
        read_or_empty(&self.paths.etc_dir.join(file.file_name()))
    }

    fn append_lines(&self, file: AccountFile, lines: &[String]) -> Result<(), AppError> {
        if lines.is_empty() {
            return Ok(());
        }
        let path = self.paths.etc_dir.join(file.file_name());
        let meta = fs::metadata(&path).map_err(|e| AppError::failed(format!("stat {}: {e}", path.display())))?;
        let mut body = read_text(&path)?;
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        atomic_write(&path, body.as_bytes(), meta.permissions().mode() & 0o7777, Some((meta.uid(), meta.gid())))
            .map_err(|e| AppError::failed(format!("write {}: {e}", path.display())))?;
        info!("accounts: appended {} line(s) to {}", lines.len(), file.file_name());
        Ok(())
    }

    fn list_upper(&self) -> Result<Vec<String>, AppError> {
        sorted_names(&self.paths.upper_dir)
    }

    fn list_key_files(&self) -> Result<Vec<String>, AppError> {
        sorted_names(&self.paths.keys_dir)
    }

    fn delete_key_file(&self, name: &str) -> Result<(), AppError> {
        if !is_safe_account_name(name) {
            return Err(AppError::invalid("not an account name"));
        }
        let path = self.paths.keys_dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => Err(AppError::invalid(format!("{} is a directory", path.display()))),
            Ok(_) => fs::remove_file(&path).map_err(|e| AppError::failed(format!("remove {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::failed(format!("stat {}: {e}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every command and answers from a script (default status 0).
    type Call = (String, Vec<String>, Vec<u8>);

    struct FakeRunner {
        calls: Mutex<Vec<Call>>,
        statuses: Mutex<Vec<i32>>,
        on_kill: Option<Box<dyn Fn() + Send + Sync>>,
        stdout: Mutex<Vec<u8>>,
    }

    impl FakeRunner {
        fn new(statuses: &[i32]) -> FakeRunner {
            FakeRunner { calls: Mutex::new(Vec::new()), statuses: Mutex::new(statuses.to_vec()), on_kill: None, stdout: Mutex::new(Vec::new()) }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<i32, AppError> {
            let args = spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            self.calls.lock().unwrap().push((spec.program.to_string(), args, spec.stdin.clone()));
            if spec.program == KILL {
                if let Some(f) = &self.on_kill {
                    f();
                }
            }
            let mut s = self.statuses.lock().unwrap();
            Ok(if s.is_empty() { 0 } else { s.remove(0) })
        }

        fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError> {
            let code = self.run(spec)?;
            Ok((code, self.stdout.lock().unwrap().clone()))
        }
    }

    impl Runner for &FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<i32, AppError> {
            (**self).run(spec)
        }

        fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError> {
            (**self).run_capture(spec)
        }
    }

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("controller-accounts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn accounts<'a>(r: &'a FakeRunner, dir: &Path) -> ShadowAccounts<&'a FakeRunner> {
        ShadowAccounts::with_runner(r, 65536).with_paths(dir.join("passwd"), dir.join("shadow"), dir.join("proc"))
    }

    #[test]
    fn set_password_argv_and_stdin_are_exact() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("setpw");
        accounts(&r, &dir).set_password("kiosk", &Secret::new("Abcdefgh12345678")).unwrap();
        let calls = r.calls();
        assert_eq!(calls.len(), 1);
        let (prog, args, stdin) = &calls[0];
        assert_eq!(prog, "/usr/sbin/chpasswd");
        assert_eq!(args, &["--crypt-method", "SHA512", "--sha-rounds", "65536"]);
        assert_eq!(stdin.as_slice(), b"kiosk:Abcdefgh12345678\n");
        // The password is nowhere in argv.
        assert!(args.iter().all(|a| !a.contains("Abcdefgh")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_password_refuses_bad_input_without_running_anything() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("badin");
        let a = accounts(&r, &dir);
        for bad in ["", "a:b", "root name", "Kiosk", "x\ny", "-r", "a".repeat(40).as_str()] {
            assert_eq!(a.set_password(bad, &Secret::new("Abcdefgh12345678")), Err(AccountsError::InvalidName), "{bad:?}");
        }
        for bad in ["", "abc\ndef:root", "abc\rdef", "ab\0cd"] {
            assert_eq!(a.set_password("kiosk", &Secret::new(bad)), Err(AccountsError::InvalidPassword), "{bad:?}");
        }
        assert!(r.calls().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_chpasswd_is_an_error_that_does_not_echo_the_password() {
        let r = FakeRunner::new(&[1]);
        let dir = tempdir("failpw");
        let err = accounts(&r, &dir).set_password("kiosk", &Secret::new("Abcdefgh12345678")).unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(shown.contains("chpasswd") && shown.contains('1'), "{shown}");
        assert!(!shown.contains("Abcdefgh"), "{shown}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_spec_debug_hides_stdin() {
        let mut s = CommandSpec::new(CHPASSWD, &["-x"]);
        s.stdin = b"kiosk:SECRETSECRET\n".to_vec();
        let shown = format!("{s:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("stdin_len"));
    }

    #[test]
    fn clear_password_argv_is_exact_and_refuses_bad_names() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("clear");
        let a = accounts(&r, &dir);
        a.clear_password("kiosk").unwrap();
        assert_eq!(
            r.calls(),
            vec![("/usr/sbin/usermod".to_string(), vec!["--password".to_string(), "!".to_string(), "kiosk".to_string()], vec![])]
        );
        assert_eq!(a.clear_password("-r"), Err(AccountsError::InvalidName));
        assert_eq!(r.calls().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_argv_is_exact() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("lock");
        accounts(&r, &dir).set_locked("alice", true).unwrap();
        assert_eq!(r.calls(), vec![("/usr/sbin/usermod".to_string(), vec!["--lock".to_string(), "--expiredate".to_string(), "1".to_string(), "alice".to_string()], vec![])]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unlock_argv_is_exact_with_an_empty_expiredate() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("unlock");
        fs::write(dir.join("shadow"), "alice:!$6$salt$hash:20000:0:99999:7:::\n").unwrap();
        accounts(&r, &dir).set_locked("alice", false).unwrap();
        assert_eq!(
            r.calls(),
            vec![("/usr/sbin/usermod".to_string(), vec!["--unlock".to_string(), "--expiredate".to_string(), String::new(), "alice".to_string()], vec![])]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unlock_of_an_account_without_a_password_is_refused_before_usermod() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("nopw");
        fs::write(dir.join("shadow"), "a:!:1::::::\nb::1::::::\nc:*:1::::::\nd:!*:1::::::\ne:!!:1::::::\n").unwrap();
        let a = accounts(&r, &dir);
        for u in ["a", "b", "c", "d", "e"] {
            assert_eq!(a.set_locked(u, false), Err(AccountsError::NoPassword), "{u}");
        }
        assert_eq!(a.set_locked("ghost", false), Err(AccountsError::NotFound));
        assert!(r.calls().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    fn call(prog: &str, args: &[&str]) -> Call {
        (prog.to_string(), args.iter().map(|a| a.to_string()).collect(), vec![])
    }

    #[test]
    fn add_user_argv_is_exact_with_no_password_and_no_shell() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("adduser");
        accounts(&r, &dir).add_user("carol").unwrap();
        assert_eq!(
            r.calls(),
            vec![call(
                "/usr/sbin/useradd",
                &["--create-home", "--home-dir", "/var/home/carol", "--shell", "/usr/bin/bash", "--user-group", "--comment", "", "carol"]
            )]
        );
        let custom = accounts(&r, &dir).with_dirs("/srv/homes", &dir);
        custom.add_user("dave").unwrap();
        assert_eq!(r.calls()[1].1[2], "/srv/homes/dave");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_user_argv_status_mapping_and_keep_home() {
        let r = FakeRunner::new(&[0, 0, 8, 6, 12]);
        let dir = tempdir("rmuser");
        let a = accounts(&r, &dir);
        a.remove_user("carol", false).unwrap();
        a.remove_user("carol", true).unwrap();
        assert_eq!(a.remove_user("carol", false), Err(AccountsError::Busy));
        assert_eq!(a.remove_user("carol", false), Err(AccountsError::NotFound));
        assert!(matches!(a.remove_user("carol", false), Err(AccountsError::Tool(_))));
        let calls = r.calls();
        assert_eq!(calls[0], call("/usr/sbin/userdel", &["--remove", "carol"]));
        assert_eq!(calls[1], call("/usr/sbin/userdel", &["carol"]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_and_member_argv_are_exact() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("groups");
        let a = accounts(&r, &dir);
        a.add_group("crew").unwrap();
        a.remove_group("crew").unwrap();
        a.add_member("alice", "crew").unwrap();
        a.remove_member("alice", "crew").unwrap();
        assert_eq!(
            r.calls(),
            vec![
                call("/usr/sbin/groupadd", &["crew"]),
                call("/usr/sbin/groupdel", &["crew"]),
                call("/usr/bin/gpasswd", &["--add", "alice", "crew"]),
                call("/usr/bin/gpasswd", &["--delete", "alice", "crew"]),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn admin_ops_refuse_option_like_and_malformed_names_without_running_anything() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("adminbad");
        let a = accounts(&r, &dir);
        for bad in ["-r", "", "a b", "Alice", "a:b", "../x", "a\nb"] {
            assert_eq!(a.add_user(bad), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.remove_user(bad, false), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.add_group(bad), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.remove_group(bad), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.add_member(bad, "crew"), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.add_member("alice", bad), Err(AccountsError::InvalidName), "{bad:?}");
            assert_eq!(a.remove_member("alice", bad), Err(AccountsError::InvalidName), "{bad:?}");
        }
        assert!(r.calls().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kiosk_terminal_flag_and_unit_follow_the_lock() {
        let r = FakeRunner::new(&[]);
        let dir = tempdir("kioskterm");
        let a = accounts(&r, &dir).with_dirs(dir.join("home"), &dir);
        let flag = dir.join("kiosk.locked");
        a.set_kiosk_terminal(false).unwrap();
        assert!(flag.is_file());
        assert_eq!(r.calls().last().unwrap(), &call("/usr/bin/systemctl", &["stop", "thing-revived-terminal-agent.service"]));
        a.set_kiosk_terminal(true).unwrap();
        assert!(!flag.exists());
        assert_eq!(r.calls().last().unwrap(), &call("/usr/bin/systemctl", &["start", "thing-revived-terminal-agent.service"]));
        a.set_kiosk_terminal(true).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no temp file left");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_capture_returns_stdout_and_run_does_not() {
        let _guard = crate::sync_util::lock(&crate::sync_util::SPAWN_GUARD);
        let spec = CommandSpec::new("/bin/echo", &["hello"]);
        let (code, out) = SystemRunner::new().run_capture(&spec).unwrap();
        assert_eq!((code, out.as_slice()), (0, b"hello\n".as_slice()));
        assert_eq!(SystemRunner::new().run(&spec).unwrap(), 0);
    }

    #[test]
    fn hash_usability() {
        assert!(hash_is_usable("$6$s$h"));
        assert!(hash_is_usable("!$6$s$h"));
        assert!(!hash_is_usable(""));
        assert!(!hash_is_usable("!"));
        assert!(!hash_is_usable("*"));
        assert!(!hash_is_usable("!*"));
        assert!(!hash_is_usable("!!"));
    }

    fn fake_proc(dir: &Path, procs: &[(u32, u32, char)]) {
        let proc_dir = dir.join("proc");
        let _ = fs::remove_dir_all(&proc_dir);
        for (pid, uid, state) in procs {
            let p = proc_dir.join(pid.to_string());
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("status"), format!("Name:\tx\nState:\t{state} (x)\nUid:\t{uid}\t0\t0\t0\nGid:\t{uid}\t{uid}\t{uid}\t{uid}\n")).unwrap();
        }
        fs::create_dir_all(proc_dir.join("self")).unwrap();
        fs::create_dir_all(proc_dir.join("sys")).unwrap();
    }

    #[test]
    fn end_sessions_runs_loginctl_then_kills_by_real_uid_then_rescans() {
        let dir = tempdir("end");
        fs::write(dir.join("passwd"), "root:x:0:0::/root:/bin/sh\nalice:x:1001:1001::/var/home/alice:/usr/bin/bash\n").unwrap();
        // 20 and 31 are alice's (31 is a setuid child: real uid 1001, euid 0),
        // 25 is a zombie of alice, 99 belongs to someone else.
        fake_proc(&dir, &[(20, 1001, 'S'), (25, 1001, 'Z'), (31, 1001, 'S'), (99, 1000, 'S')]);
        let proc_dir = dir.join("proc");
        let mut r = FakeRunner::new(&[1, 0]);
        r.on_kill = Some(Box::new(move || {
            let _ = fs::remove_dir_all(proc_dir.join("20"));
            let _ = fs::remove_dir_all(proc_dir.join("31"));
        }));
        accounts(&r, &dir).end_sessions("alice").unwrap();
        let calls = r.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].0.as_str(), calls[0].1.clone()), ("/usr/bin/loginctl", vec!["terminate-user".to_string(), "alice".to_string()]));
        assert_eq!((calls[1].0.as_str(), calls[1].1.clone()), ("/usr/bin/kill", vec!["-KILL".to_string(), "20".to_string(), "31".to_string()]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loginctl_status_one_is_tolerated_and_no_processes_means_no_kill() {
        let dir = tempdir("nologin");
        fs::write(dir.join("passwd"), "alice:x:1001:1001::/h:/bin/sh\n").unwrap();
        fake_proc(&dir, &[(99, 1000, 'S')]);
        let r = FakeRunner::new(&[1]);
        accounts(&r, &dir).end_sessions("alice").unwrap();
        assert_eq!(r.calls().len(), 1);
        assert_eq!(r.calls()[0].0, "/usr/bin/loginctl");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn survivors_are_reported_as_sessions_remain() {
        let dir = tempdir("remain");
        fs::write(dir.join("passwd"), "alice:x:1001:1001::/h:/bin/sh\n").unwrap();
        fake_proc(&dir, &[(20, 1001, 'S')]);
        let r = FakeRunner::new(&[1, 1]);
        assert_eq!(accounts(&r, &dir).end_sessions("alice"), Err(AccountsError::SessionsRemain));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn end_sessions_never_targets_root_or_unknown_users() {
        let dir = tempdir("rootkill");
        fs::write(dir.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
        fake_proc(&dir, &[(1, 0, 'S')]);
        let r = FakeRunner::new(&[]);
        let a = accounts(&r, &dir);
        assert_eq!(a.end_sessions("root"), Err(AccountsError::InvalidName));
        assert_eq!(a.end_sessions("ghost"), Err(AccountsError::NotFound));
        assert!(r.calls().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_parsing() {
        assert!(status_matches("State:\tS (sleeping)\nUid:\t1001\t0\t0\t0\n", 1001));
        assert!(!status_matches("State:\tZ (zombie)\nUid:\t1001\t1001\t1001\t1001\n", 1001));
        assert!(!status_matches("State:\tS (sleeping)\nUid:\t0\t1001\t1001\t1001\n", 1001));
        assert!(!status_matches("garbage", 1001));
    }

    #[test]
    fn the_system_runner_clears_the_environment_and_passes_stdin() {
        let _guard = crate::sync_util::lock(&crate::sync_util::SPAWN_GUARD);
        // /usr/bin/env prints its environment: only PATH and LANG may exist.
        let dir = tempdir("sysrun");
        let script = dir.join("probe.sh");
        fs::write(&script, "#!/bin/sh\n/usr/bin/env > \"$1\"\ncat > \"$2\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let script: &'static str = Box::leak(script.to_string_lossy().into_owned().into_boxed_str());
        std::env::set_var("SECRET_PROBE_VAR", "leak");
        let mut spec = CommandSpec::new(script, &[]);
        spec.args = vec![dir.join("env.txt").into(), dir.join("in.txt").into()];
        spec.stdin = b"user:pw\n".to_vec();
        assert_eq!(SystemRunner::new().run(&spec).unwrap(), 0);
        let env = fs::read_to_string(dir.join("env.txt")).unwrap();
        let mut keys: Vec<&str> = env.lines().filter_map(|l| l.split('=').next()).collect();
        keys.sort_unstable();
        assert!(keys.iter().all(|k| ["LANG", "PATH", "PWD", "SHLVL", "_", "OLDPWD"].contains(k)), "{keys:?}");
        assert!(env.contains("PATH=/usr/sbin:/usr/bin:/sbin:/bin") && env.contains("LANG=C"), "{env}");
        assert_eq!(fs::read(dir.join("in.txt")).unwrap(), b"user:pw\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_runner_reports_exit_status_and_spawn_failure() {
        let _guard = crate::sync_util::lock(&crate::sync_util::SPAWN_GUARD);
        assert_eq!(SystemRunner::new().run(&CommandSpec::new("/bin/false", &[])).unwrap(), 1);
        let err = SystemRunner::new().run(&CommandSpec::new("/nonexistent/tool", &[])).unwrap_err();
        assert!(err.message.contains("/nonexistent/tool"), "{err}");
    }

    #[test]
    fn the_system_runner_kills_a_hung_tool() {
        let _guard = crate::sync_util::lock(&crate::sync_util::SPAWN_GUARD);
        let runner = SystemRunner { timeout: Duration::from_millis(100) };
        let err = runner.run(&CommandSpec::new("/bin/sleep", &["30"])).unwrap_err();
        assert!(err.message.contains("timed out"), "{err}");
    }

    #[test]
    fn state_file_round_trip_0600_and_missing_is_none_and_corrupt_is_unacknowledged() {
        let dir = tempdir("state").join("accounts");
        let s = JsonStateFile::new(&dir);
        assert_eq!(s.load().unwrap(), None);
        let st = AccountState { epoch: "ab".repeat(16), kiosk_password_set: true, acknowledged: false };
        s.save(&st).unwrap();
        assert_eq!(s.load().unwrap(), Some(st.clone()));
        assert_eq!(fs::metadata(s.path()).unwrap().permissions().mode() & 0o777, 0o600);
        let on_disk = fs::read_to_string(s.path()).unwrap();
        assert!(on_disk.contains("\"kiosk_password_set\": true") && on_disk.contains("\"acknowledged\": false"), "{on_disk}");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temp file left");
        s.save(&st.acknowledged()).unwrap();
        assert!(s.load().unwrap().unwrap().acknowledged);
        fs::write(s.path(), "{not json").unwrap();
        let corrupt = s.load().unwrap().expect("corrupt is not missing");
        assert!(!corrupt.acknowledged && corrupt.kiosk_password_set);
        fs::write(s.path(), "").unwrap();
        assert!(!s.load().unwrap().unwrap().acknowledged, "an empty file is corrupt too");
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    fn etc_fixture(name: &str) -> (PathBuf, EtcFiles) {
        let root = tempdir(name);
        let paths = EtcPaths {
            etc_dir: root.join("etc"),
            lower_dir: root.join("lower"),
            upper_dir: root.join("upper"),
            keys_dir: root.join("keys"),
            mountinfo: root.join("mountinfo"),
        };
        for d in [&paths.etc_dir, &paths.lower_dir, &paths.upper_dir, &paths.keys_dir] {
            fs::create_dir_all(d).unwrap();
        }
        (root, EtcFiles::new(paths))
    }

    #[test]
    fn mountinfo_detection_needs_an_overlay_exactly_on_etc() {
        let ov = "41 29 0:35 / /etc rw,relatime shared:1 - overlay overlay rw,lowerdir=/run/x,upperdir=/var/y\n";
        assert!(overlay_mounted_on(ov, "/etc"));
        assert!(!overlay_mounted_on(ov, "/etc/ssh"));
        let ro_bind = "41 29 8:3 /etc /etc ro,relatime - ext4 /dev/mmcblk0p3 ro\n";
        assert!(!overlay_mounted_on(ro_bind, "/etc"));
        let machine_id = "50 41 0:40 / /etc/machine-id rw - tmpfs tmpfs rw\n";
        assert!(!overlay_mounted_on(machine_id, "/etc"));
        assert!(!overlay_mounted_on("garbage", "/etc"));
        assert!(!overlay_mounted_on("", "/etc"));
    }

    #[test]
    fn storage_is_unavailable_without_the_overlay_even_if_the_lower_bind_exists() {
        let (root, files) = etc_fixture("avail");
        fs::write(files.paths.lower_dir.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
        assert!(!files.storage_available());
        let etc = files.paths.etc_dir.display();
        fs::write(&files.paths.mountinfo, format!("41 29 8:3 /etc {etc} ro - ext4 /dev/x ro\n")).unwrap();
        assert!(!files.storage_available());
        fs::write(&files.paths.mountinfo, format!("41 29 0:35 / {etc} rw - overlay overlay rw\n")).unwrap();
        assert!(files.storage_available());
        fs::remove_file(files.paths.lower_dir.join("passwd")).unwrap();
        assert!(!files.storage_available());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn append_lines_keeps_mode_adds_a_missing_newline_and_leaves_no_temp() {
        let (root, files) = etc_fixture("append");
        let shadow = files.paths.etc_dir.join("shadow");
        fs::write(&shadow, "root:*:1::::::").unwrap();
        fs::set_permissions(&shadow, fs::Permissions::from_mode(0o640)).unwrap();
        files.append_lines(AccountFile::Shadow, &["newsvc:*:1::::::".to_string()]).unwrap();
        assert_eq!(fs::read_to_string(&shadow).unwrap(), "root:*:1::::::\nnewsvc:*:1::::::\n");
        assert_eq!(fs::metadata(&shadow).unwrap().permissions().mode() & 0o7777, 0o640);
        assert_eq!(fs::read_dir(&files.paths.etc_dir).unwrap().count(), 1);
        files.append_lines(AccountFile::Shadow, &[]).unwrap();
        assert_eq!(fs::read_to_string(&shadow).unwrap().lines().count(), 2);
        assert!(files.append_lines(AccountFile::Passwd, &["x".to_string()]).is_err(), "missing file is an error");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_store_listing_and_deletion_are_confined_to_account_names() {
        let (root, files) = etc_fixture("keys");
        for n in ["alice", "bob"] {
            fs::write(files.paths.keys_dir.join(n), "ssh-ed25519 AAAA\n").unwrap();
        }
        fs::create_dir(files.paths.keys_dir.join("subdir")).unwrap();
        assert_eq!(files.list_key_files().unwrap(), vec!["alice", "bob", "subdir"]);
        files.delete_key_file("bob").unwrap();
        files.delete_key_file("bob").unwrap();
        assert!(!files.paths.keys_dir.join("bob").exists());
        for bad in ["../etc/passwd", "a/b", "", "Alice", "subdir"] {
            let _ = files.delete_key_file(bad);
        }
        assert!(files.paths.keys_dir.join("subdir").is_dir());
        assert!(files.delete_key_file("../x").is_err());
        assert!(files.paths.keys_dir.join("alice").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn upper_listing() {
        let (root, files) = etc_fixture("upper");
        fs::write(files.paths.upper_dir.join("passwd"), "").unwrap();
        fs::write(files.paths.upper_dir.join("hostname"), "").unwrap();
        assert_eq!(files.list_upper().unwrap(), vec!["hostname", "passwd"]);
        let gone = EtcFiles::new(EtcPaths { upper_dir: root.join("nope"), ..files.paths.clone() });
        assert_eq!(gone.list_upper().unwrap(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&root);
    }
}
