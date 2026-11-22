//! `SecretStore` on a tmpfs file (`/run/thing-revived/first-boot-secret`,
//! 0600 root) and the monotonic clock the expiry in it is measured against.
//!
//! File format: the expiry in whole monotonic seconds, a newline, the
//! password, a newline. The password is never logged and never in an error.

use crate::app::error::AppError;
use crate::domain::accounts::password::Secret;
use crate::ports::{MonotonicClock, SecretStore, StoredSecret};
use log::warn;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const SECRET_FILE: &str = "first-boot-secret";

pub struct TmpfsSecretStore {
    dir: PathBuf,
}

impl TmpfsSecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> TmpfsSecretStore {
        TmpfsSecretStore { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(SECRET_FILE)
    }

    fn tmp_path(&self) -> PathBuf {
        self.dir.join(format!("{SECRET_FILE}.tmp"))
    }
}

/// Overwrites the file with zeros of the same length, syncs, then unlinks.
/// A missing file is fine; a failed overwrite still attempts the unlink.
fn scrub_file(path: &Path) -> Result<(), AppError> {
    let len = match fs::symlink_metadata(path) {
        Ok(m) => m.len() as usize,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::failed(format!("stat {}: {e}", path.display()))),
    };
    let wiped = OpenOptions::new().write(true).open(path).and_then(|mut f| {
        f.write_all(&vec![0u8; len])?;
        f.sync_all()
    });
    let removed = fs::remove_file(path);
    if let Err(e) = wiped {
        warn!("secret: could not overwrite {} before removal: {e}", path.display());
    }
    match removed {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::failed(format!("remove {}: {e}", path.display()))),
    }
}

impl SecretStore for TmpfsSecretStore {
    fn store(&self, secret: &Secret, expires_at_secs: u64) -> Result<(), AppError> {
        DirBuilder::new().recursive(true).mode(0o700).create(&self.dir).map_err(|e| AppError::failed(format!("create {}: {e}", self.dir.display())))?;
        let tmp = self.tmp_path();
        let mut body = format!("{expires_at_secs}\n{}\n", secret.expose()).into_bytes();
        let result = (|| {
            // Fresh file every time: a stale temp with looser bits is removed.
            let _ = fs::remove_file(&tmp);
            let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
            f.write_all(&body)?;
            f.sync_all()?;
            fs::rename(&tmp, self.path())
        })();
        body.iter_mut().for_each(|b| *b = 0);
        std::hint::black_box(&body);
        if let Err(e) = result {
            let _ = scrub_file(&tmp);
            return Err(AppError::failed(format!("write {}: {e}", self.path().display())));
        }
        Ok(())
    }

    fn load(&self) -> Result<Option<StoredSecret>, AppError> {
        let path = self.path();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AppError::failed(format!("read {}: {e}", path.display()))),
        };
        let mut it = text.lines();
        let parsed = it.next().and_then(|l| l.parse::<u64>().ok()).zip(it.next().filter(|p| !p.is_empty()));
        match parsed {
            Some((expires_at_secs, pw)) => Ok(Some(StoredSecret { secret: Secret::new(pw), expires_at_secs })),
            None => {
                warn!("secret: {} is malformed, ignoring it", path.display());
                Ok(None)
            }
        }
    }

    fn scrub(&self) -> Result<(), AppError> {
        let main = scrub_file(&self.path());
        let tmp = scrub_file(&self.tmp_path());
        main.and(tmp)
    }
}

/// Seconds since boot from `/proc/uptime` (CLOCK_BOOTTIME). Used instead of
/// `Instant` because the expiry must be comparable across processes: the
/// provision run writes it and the controller service reads it.
pub struct ProcUptime {
    path: PathBuf,
}

impl ProcUptime {
    pub fn new() -> ProcUptime {
        ProcUptime { path: PathBuf::from("/proc/uptime") }
    }

    pub fn with_path(path: impl Into<PathBuf>) -> ProcUptime {
        ProcUptime { path: path.into() }
    }
}

impl Default for ProcUptime {
    fn default() -> Self {
        ProcUptime::new()
    }
}

/// First field of `/proc/uptime`, truncated to whole seconds.
fn parse_uptime(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.split('.').next()?.parse().ok()
}

impl MonotonicClock for ProcUptime {
    fn now_secs(&self) -> u64 {
        // An unreadable clock reads as 0: a deadline computed from it is then
        // `ttl`, which only means the screen outlives its window by a boot.
        fs::read_to_string(&self.path).ok().and_then(|t| parse_uptime(&t)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("controller-secret-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn store_writes_0600_and_load_round_trips() {
        let dir = tempdir("rt").join("run");
        let s = TmpfsSecretStore::new(&dir);
        s.store(&Secret::new("Abcdefgh12345678"), 1234).unwrap();
        let meta = fs::metadata(s.path()).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::read_to_string(s.path()).unwrap(), "1234\nAbcdefgh12345678\n");
        let got = s.load().unwrap().unwrap();
        assert_eq!(got.secret.expose(), "Abcdefgh12345678");
        assert_eq!(got.expires_at_secs, 1234);
        assert!(!s.tmp_path().exists());
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn store_replaces_a_previous_secret() {
        let dir = tempdir("replace");
        let s = TmpfsSecretStore::new(&dir);
        s.store(&Secret::new("Firstfirst1234"), 10).unwrap();
        s.store(&Secret::new("Secondsecond5678"), 20).unwrap();
        let got = s.load().unwrap().unwrap();
        assert_eq!((got.secret.expose(), got.expires_at_secs), ("Secondsecond5678", 20));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_loose_temp_file_does_not_leak_through() {
        let dir = tempdir("stale");
        let s = TmpfsSecretStore::new(&dir);
        fs::write(s.tmp_path(), "junk").unwrap();
        fs::set_permissions(s.tmp_path(), fs::Permissions::from_mode(0o666)).unwrap();
        s.store(&Secret::new("Abcdefgh12345678"), 5).unwrap();
        assert_eq!(fs::metadata(s.path()).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scrub_removes_the_file_and_is_idempotent() {
        let dir = tempdir("scrub");
        let s = TmpfsSecretStore::new(&dir);
        s.store(&Secret::new("Abcdefgh12345678"), 5).unwrap();
        s.scrub().unwrap();
        assert!(!s.path().exists());
        assert!(s.load().unwrap().is_none());
        s.scrub().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scrub_overwrites_before_unlinking() {
        let dir = tempdir("wipe");
        let path = dir.join("victim");
        fs::write(&path, "1\nAbcdefgh12345678\n").unwrap();
        // Keep a second name for the same inode: after scrub the surviving
        // link must show zeros, proving the content was overwritten in place.
        let alias = dir.join("alias");
        fs::hard_link(&path, &alias).unwrap();
        scrub_file(&path).unwrap();
        assert!(!path.exists());
        let left = fs::read(&alias).unwrap();
        assert_eq!(left.len(), 19);
        assert!(left.iter().all(|b| *b == 0), "{left:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_reads_as_absent() {
        let dir = tempdir("bad");
        let s = TmpfsSecretStore::new(&dir);
        for body in ["", "nope\nx\n", "12\n", "12\n\n", "justone"] {
            fs::write(s.path(), body).unwrap();
            assert!(s.load().unwrap().is_none(), "{body:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_of_nothing_is_none() {
        let s = TmpfsSecretStore::new(tempdir("none").join("missing"));
        assert!(s.load().unwrap().is_none());
    }

    #[test]
    fn errors_never_contain_the_password() {
        // The directory is a file, so store fails; the message names paths only.
        let dir = tempdir("err");
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let s = TmpfsSecretStore::new(blocker.join("sub"));
        let err = s.store(&Secret::new("Abcdefgh12345678"), 1).unwrap_err();
        assert!(!err.message.contains("Abcdefgh"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uptime_parsing() {
        assert_eq!(parse_uptime("351735.47 1234.5\n"), Some(351735));
        assert_eq!(parse_uptime("12 3"), Some(12));
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("x.1"), None);
        let d = tempdir("up");
        fs::write(d.join("uptime"), "42.9 1.0\n").unwrap();
        assert_eq!(ProcUptime::with_path(d.join("uptime")).now_secs(), 42);
        assert_eq!(ProcUptime::with_path(d.join("absent")).now_secs(), 0);
        assert!(ProcUptime::new().now_secs() > 0);
        let _ = fs::remove_dir_all(&d);
    }
}
