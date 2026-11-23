//! `AuthorizedKeysStore` on the menu-managed key directory
//! (`/var/lib/thing-revived-ssh/authorized_keys/<user>`, sshd's
//! `AuthorizedKeysFile`).
//!
//! Each file is written atomically (temp file in the same directory, fsync,
//! rename, fsync the directory), mode 0600 and owned by the account it
//! belongs to: sshd opens it as the target user, and `StrictModes` accepts a
//! file owned by that user (or root) that nobody else can write.

use crate::adapters::accounts_shadow::atomic_write;
use crate::app::error::AppError;
use crate::domain::accounts::state::is_safe_account_name;
use crate::ports::AuthorizedKeysStore;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::{Path, PathBuf};

pub struct AuthorizedKeysFs {
    dir: PathBuf,
}

impl AuthorizedKeysFs {
    pub fn new(dir: impl Into<PathBuf>) -> AuthorizedKeysFs {
        AuthorizedKeysFs { dir: dir.into() }
    }

    fn path_of(&self, user: &str) -> Result<PathBuf, AppError> {
        if !is_safe_account_name(user) {
            return Err(AppError::invalid("not an account name"));
        }
        Ok(self.dir.join(user))
    }
}

fn describe(path: &Path) -> String {
    path.display().to_string()
}

impl AuthorizedKeysStore for AuthorizedKeysFs {
    fn read(&self, user: &str) -> Result<Vec<String>, AppError> {
        let path = self.path_of(user)?;
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() || m.is_dir() || m.file_type().is_fifo() => {
                return Err(AppError::failed(format!("{} is not a regular file", describe(&path))));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppError::failed(format!("stat {}: {e}", describe(&path)))),
        }
        let text = fs::read_to_string(&path).map_err(|e| AppError::failed(format!("read {}: {e}", describe(&path))))?;
        Ok(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect())
    }

    fn write(&self, user: &str, lines: &[String], owner: (u32, u32)) -> Result<(), AppError> {
        let path = self.path_of(user)?;
        if lines.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(AppError::failed(format!("remove {}: {e}", describe(&path)))),
            };
        }
        if lines.iter().any(|l| l.contains(['\n', '\r', '\0'])) {
            return Err(AppError::invalid("a key line must be a single line"));
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(&self.dir)
            .map_err(|e| AppError::failed(format!("create {}: {e}", describe(&self.dir))))?;
        let mut body = lines.join("\n");
        body.push('\n');
        atomic_write(&path, body.as_bytes(), 0o600, Some(owner)).map_err(|e| AppError::failed(format!("write {}: {e}", describe(&path))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn fixture(name: &str) -> (PathBuf, AuthorizedKeysFs, (u32, u32)) {
        let root = std::env::temp_dir().join(format!("controller-keysfs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let meta = fs::metadata(&root).unwrap();
        (root.clone(), AuthorizedKeysFs::new(root.join("keys")), (meta.uid(), meta.gid()))
    }

    #[test]
    fn write_is_atomic_0600_owned_and_creates_the_directory() {
        let (root, store, owner) = fixture("write");
        let lines = vec!["ssh-ed25519 AAAA one".to_string(), "ssh-rsa BBBB two".to_string()];
        store.write("alice", &lines, owner).unwrap();
        let path = root.join("keys/alice");
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
        assert_eq!((meta.uid(), meta.gid()), owner);
        assert_eq!(fs::read_to_string(&path).unwrap(), "ssh-ed25519 AAAA one\nssh-rsa BBBB two\n");
        assert_eq!(fs::metadata(root.join("keys")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(fs::read_dir(root.join("keys")).unwrap().count(), 1, "no temp file left");
        assert_eq!(store.read("alice").unwrap(), lines);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rewrite_replaces_and_a_stale_temp_does_not_block() {
        let (root, store, owner) = fixture("rewrite");
        store.write("alice", &["ssh-ed25519 AAAA one".to_string()], owner).unwrap();
        fs::write(root.join("keys/alice.thing-revived.tmp"), "stale").unwrap();
        store.write("alice", &["ssh-ed25519 CCCC three".to_string()], owner).unwrap();
        assert_eq!(store.read("alice").unwrap(), vec!["ssh-ed25519 CCCC three".to_string()]);
        assert_eq!(fs::metadata(root.join("keys/alice")).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_no_lines_removes_the_file_and_is_idempotent() {
        let (root, store, owner) = fixture("empty");
        store.write("alice", &["ssh-ed25519 AAAA one".to_string()], owner).unwrap();
        store.write("alice", &[], owner).unwrap();
        assert!(!root.join("keys/alice").exists());
        store.write("alice", &[], owner).unwrap();
        assert_eq!(store.read("alice").unwrap(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn names_are_confined_and_lines_must_be_single() {
        let (root, store, owner) = fixture("names");
        for bad in ["../x", "a/b", "", "Alice", "-r"] {
            assert!(store.read(bad).is_err(), "{bad:?}");
            assert!(store.write(bad, &["ssh-ed25519 A".to_string()], owner).is_err(), "{bad:?}");
        }
        assert!(store.write("alice", &["a\nb".to_string()], owner).is_err());
        assert!(!root.join("keys").exists() || fs::read_dir(root.join("keys")).unwrap().count() == 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_skips_comments_and_blank_lines_and_refuses_a_symlink() {
        let (root, store, owner) = fixture("read");
        fs::create_dir_all(root.join("keys")).unwrap();
        fs::write(root.join("keys/alice"), "# note\n\nssh-ed25519 AAAA one\n  \n").unwrap();
        assert_eq!(store.read("alice").unwrap(), vec!["ssh-ed25519 AAAA one".to_string()]);
        std::os::unix::fs::symlink("/etc/passwd", root.join("keys/bob")).unwrap();
        assert!(store.read("bob").is_err());
        let _ = owner;
        let _ = fs::remove_dir_all(&root);
    }
}
