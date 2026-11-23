//! `PasswordVerifier` over `/sbin/unix_chkpwd <user> nonull`.
//!
//! The password and a NUL byte go to the helper's stdin (never argv or the
//! environment); the exit status is the answer. `nonull`, never `nullok`, so
//! an empty password cannot match an empty hash. The adapter also refuses,
//! without spawning anything, every account whose `/etc/shadow` entry has no
//! usable hash or carries a lock prefix, so the verdict does not depend on
//! how the helper treats those.
//!
//! This path bypasses PAM and therefore pam_faillock: the controller's own
//! unlock rate limiter is the only brake on guesses made through it.

use crate::adapters::accounts_shadow::{CommandSpec, Runner, SystemRunner};
use crate::app::error::AppError;
use crate::domain::accounts::password::Secret;
use crate::domain::accounts::state::{hash_is_usable, is_safe_account_name, shadow_hash};
use crate::ports::PasswordVerifier;
use std::fs;
use std::path::PathBuf;

pub const UNIX_CHKPWD: &str = "/sbin/unix_chkpwd";
/// The helper reads at most `PAM_MAX_RESP_SIZE` (512) bytes.
const MAX_PASSWORD: usize = 256;

pub struct UnixChkpwd<R: Runner> {
    runner: R,
    shadow_path: PathBuf,
}

impl UnixChkpwd<SystemRunner> {
    pub fn new(runner: SystemRunner) -> UnixChkpwd<SystemRunner> {
        UnixChkpwd::with_runner(runner)
    }
}

impl<R: Runner> UnixChkpwd<R> {
    pub fn with_runner(runner: R) -> UnixChkpwd<R> {
        UnixChkpwd { runner, shadow_path: PathBuf::from("/etc/shadow") }
    }

    /// Reads another shadow file (tests).
    pub fn with_shadow(mut self, path: impl Into<PathBuf>) -> Self {
        self.shadow_path = path.into();
        self
    }
}

impl<R: Runner> PasswordVerifier for UnixChkpwd<R> {
    fn verify(&self, user: &str, password: &Secret) -> Result<bool, AppError> {
        let pw = password.expose();
        if !is_safe_account_name(user) || pw.is_empty() || pw.len() > MAX_PASSWORD || pw.contains('\0') {
            return Ok(false);
        }
        let shadow = fs::read_to_string(&self.shadow_path).map_err(|e| AppError::failed(format!("read {}: {e}", self.shadow_path.display())))?;
        // A locked (`!` prefix) or hashless account never verifies.
        let verifiable = shadow_hash(&shadow, user).is_some_and(|h| !h.starts_with('!') && hash_is_usable(h));
        if !verifiable {
            return Ok(false);
        }
        let mut spec = CommandSpec::new(UNIX_CHKPWD, &[user, "nonull"]);
        spec.stdin = pw.bytes().chain(std::iter::once(0)).collect();
        Ok(self.runner.run(&spec)? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Call = (String, Vec<String>, Vec<u8>);

    struct Fake {
        calls: Mutex<Vec<Call>>,
        status: i32,
    }

    impl Runner for &Fake {
        fn run(&self, spec: &CommandSpec) -> Result<i32, AppError> {
            let args = spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            self.calls.lock().unwrap().push((spec.program.to_string(), args, spec.stdin.clone()));
            Ok(self.status)
        }

        fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError> {
            self.run(spec).map(|c| (c, Vec::new()))
        }
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("controller-chkpwd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let shadow = dir.join("shadow");
        fs::write(
            &shadow,
            "alice:$6$salt$hash:19000:0:99999:7:::\nbob:!$6$salt$hash:19000::::::\nnopw:!:19000::::::\nstar:*:19000::::::\nempty::19000::::::\n",
        )
        .unwrap();
        shadow
    }

    #[test]
    fn argv_and_stdin_are_exact_and_the_password_is_not_in_argv() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0 };
        let shadow = fixture("argv");
        let v = UnixChkpwd::with_runner(&f).with_shadow(&shadow);
        assert!(v.verify("alice", &Secret::new("Correct-Horse-9")).unwrap());
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/sbin/unix_chkpwd");
        assert_eq!(calls[0].1, vec!["alice".to_string(), "nonull".to_string()]);
        assert_eq!(calls[0].2, b"Correct-Horse-9\0");
        assert!(calls[0].1.iter().all(|a| !a.contains("Horse")));
        let _ = fs::remove_dir_all(shadow.parent().unwrap());
    }

    #[test]
    fn any_non_zero_status_is_a_wrong_password() {
        let shadow = fixture("status");
        for status in [7, 10, 1] {
            let f = Fake { calls: Mutex::new(Vec::new()), status };
            let v = UnixChkpwd::with_runner(&f).with_shadow(&shadow);
            assert!(!(v.verify("alice", &Secret::new("wrong-password-1")).unwrap()), "{status}");
        }
        let _ = fs::remove_dir_all(shadow.parent().unwrap());
    }

    #[test]
    fn accounts_that_cannot_authenticate_never_spawn_the_helper() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0 };
        let shadow = fixture("noauth");
        let v = UnixChkpwd::with_runner(&f).with_shadow(&shadow);
        for user in ["bob", "nopw", "star", "empty", "ghost"] {
            assert!(!(v.verify(user, &Secret::new("Correct-Horse-9")).unwrap()), "{user}");
        }
        assert!(f.calls.lock().unwrap().is_empty(), "status 0 from the fake would have said yes");
        let _ = fs::remove_dir_all(shadow.parent().unwrap());
    }

    #[test]
    fn bad_names_and_passwords_are_refused_without_a_spawn() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0 };
        let shadow = fixture("badin");
        let v = UnixChkpwd::with_runner(&f).with_shadow(&shadow);
        assert!(!v.verify("-r", &Secret::new("Correct-Horse-9")).unwrap());
        assert!(!v.verify("alice", &Secret::new("")).unwrap());
        assert!(!v.verify("alice", &Secret::new("a\0b")).unwrap());
        assert!(!v.verify("alice", &Secret::new("a".repeat(300))).unwrap());
        assert!(f.calls.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(shadow.parent().unwrap());
    }

    #[test]
    fn an_unreadable_shadow_is_an_error_not_a_no() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0 };
        let v = UnixChkpwd::with_runner(&f).with_shadow("/nonexistent/shadow");
        assert!(v.verify("alice", &Secret::new("Correct-Horse-9")).is_err());
    }
}
