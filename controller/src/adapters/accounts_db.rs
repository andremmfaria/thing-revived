//! `AccountDb` over `/etc/passwd`, `/etc/group` and `/etc/shadow`. The
//! shadow file is reduced to `{locked, has_password, expire}` while parsing
//! (`domain::accounts::model`), so no hash leaves this adapter.

use crate::app::error::AppError;
use crate::domain::accounts::model::Snapshot;
use crate::domain::accounts::state::is_safe_account_name;
use crate::ports::AccountDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct EtcAccountDb {
    etc_dir: PathBuf,
    home_base: PathBuf,
    keys_dir: PathBuf,
}

impl EtcAccountDb {
    pub fn new(etc_dir: impl Into<PathBuf>, home_base: impl Into<PathBuf>, keys_dir: impl Into<PathBuf>) -> EtcAccountDb {
        EtcAccountDb { etc_dir: etc_dir.into(), home_base: home_base.into(), keys_dir: keys_dir.into() }
    }

    fn read(&self, file: &str) -> Result<String, AppError> {
        let path = self.etc_dir.join(file);
        fs::read_to_string(&path).map_err(|e| AppError::failed(format!("read {}: {e}", path.display())))
    }
}

/// True if anything (even a dangling symlink) is at `path`.
fn present(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::failed(format!("stat {}: {e}", path.display()))),
    }
}

impl AccountDb for EtcAccountDb {
    fn snapshot(&self) -> Result<Snapshot, AppError> {
        Ok(Snapshot::parse(&self.read("passwd")?, &self.read("group")?, &self.read("shadow")?))
    }

    fn today_days(&self) -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d.as_secs() / 86_400) as i64).unwrap_or(0)
    }

    fn leftover_state(&self, name: &str) -> Result<bool, AppError> {
        if !is_safe_account_name(name) {
            return Err(AppError::invalid("not an account name"));
        }
        Ok(present(&self.home_base.join(name))? || present(&self.keys_dir.join(name))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (PathBuf, EtcAccountDb) {
        let root = std::env::temp_dir().join(format!("controller-accdb-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["etc", "home", "keys"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("etc/passwd"), "root:x:0:0::/root:/bin/sh\nalice:x:1001:1001::/var/home/alice:/usr/bin/bash\n").unwrap();
        fs::write(root.join("etc/group"), "root:x:0:\nsudo:x:27:alice\nalice:x:1001:\n").unwrap();
        fs::write(root.join("etc/shadow"), "root:*:19000:0:99999:7:::\nalice:$6$salt$secrethash:19000:0:99999:7:::\n").unwrap();
        let db = EtcAccountDb::new(root.join("etc"), root.join("home"), root.join("keys"));
        (root, db)
    }

    #[test]
    fn snapshot_reads_the_three_files_and_drops_hashes() {
        let (root, db) = fixture("snap");
        let s = db.snapshot().unwrap();
        assert_eq!(s.users.len(), 2);
        assert!(s.is_in_group("alice", "sudo"));
        assert!(s.has_password("alice") && !s.has_password("root"));
        assert!(!format!("{s:?}").contains("secrethash"));
        fs::remove_file(root.join("etc/group")).unwrap();
        assert!(db.snapshot().is_err(), "a missing file is an error, never an empty snapshot");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn leftover_state_sees_a_home_or_a_key_file() {
        let (root, db) = fixture("left");
        assert!(!db.leftover_state("carol").unwrap());
        fs::create_dir(root.join("home/carol")).unwrap();
        assert!(db.leftover_state("carol").unwrap());
        fs::write(root.join("keys/dave"), "ssh-ed25519 A\n").unwrap();
        assert!(db.leftover_state("dave").unwrap());
        std::os::unix::fs::symlink("/nowhere", root.join("home/erin")).unwrap();
        assert!(db.leftover_state("erin").unwrap(), "a dangling symlink still blocks a re-create");
        assert!(db.leftover_state("../x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn today_is_a_plausible_day_number() {
        let (root, db) = fixture("days");
        assert!(db.today_days() > 19_000);
        let _ = fs::remove_dir_all(&root);
    }
}
