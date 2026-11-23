//! `controller-agent provision`: runs once per boot from
//! `thing-revived-provision.service` and is a fast no-op in steady state.
//!
//! 1. Reconcile (plan D10): append system accounts and groups the image has
//!    and the `/etc` overlay's upper copy lacks, delete key files of users
//!    that no longer exist, warn about unexpected upper files.
//! 2. Boot decision (delta plan 4.1): adopt a password that is already set,
//!    re-lock our own unacknowledged one, or leave the prompt to the
//!    controller. It never generates: the password is made at the physical M
//!    press, by the controller (`services::users`), and shown on screen.
//!
//! Everything goes through ports, so the tests below run with fakes. No
//! password exists in this process.

use controller_agent::adapters::accounts_shadow::{EtcFiles, EtcPaths, JsonStateFile, ShadowAccounts, DEFAULT_SHA_ROUNDS};
use controller_agent::adapters::os_entropy::OsEntropy;
use controller_agent::app::error::AppError;
use controller_agent::domain::accounts::state::{plan_reconcile, stale_key_files, unexpected_upper_entries, AccountFile, ReconcileInput};
use controller_agent::ports::{AccountFiles, AccountStateStore, AccountsPort, Entropy};
use controller_agent::services::users::{settle_boot, BootOutcome};
use controller_agent::utils::config::{Config, LoggingConfig};
use controller_agent::utils::logging;
use log::{error, info, warn};

/// Plan `[users]` default for the directory this command owns.
const STATE_DIR: &str = "/var/lib/thing-revived/accounts";

pub struct Ports<'a> {
    pub entropy: &'a dyn Entropy,
    pub state: &'a dyn AccountStateStore,
    pub accounts: &'a dyn AccountsPort,
    pub files: &'a dyn AccountFiles,
}

/// What the boot decision found; the same type the controller uses.
pub type Outcome = BootOutcome;

/// Process entry for the subcommand; returns the exit status.
pub fn command() -> i32 {
    // A broken controller.toml must not stop account provisioning.
    let logging_cfg = Config::load().map(|l| l.config.logging).unwrap_or_else(|_| LoggingConfig::default());
    logging::init(&logging_cfg);
    let entropy = OsEntropy::new();
    let state = JsonStateFile::new(STATE_DIR);
    let accounts = ShadowAccounts::new(DEFAULT_SHA_ROUNDS);
    let files = EtcFiles::new(EtcPaths::default());
    let ports = Ports { entropy: &entropy, state: &state, accounts: &accounts, files: &files };
    match run(&ports) {
        Ok(Outcome::StorageUnavailable) => {
            error!("provision: account storage (the /etc overlay) is not available, nothing was changed");
            1
        }
        Ok(outcome) => {
            info!("provision: done ({outcome:?})");
            0
        }
        Err(e) => {
            error!("provision: failed: {e}");
            1
        }
    }
}

pub fn run(p: &Ports<'_>) -> Result<Outcome, AppError> {
    if !p.files.storage_available() {
        return Ok(Outcome::StorageUnavailable);
    }
    // A reconcile problem is loud but must not block the boot decision: a
    // wrongly kept password is worse than a missing system entry.
    if let Err(e) = reconcile(p.files) {
        error!("provision: reconcile failed: {e}");
    }
    settle_boot(p.files, p.state, p.accounts, p.entropy)
}

fn reconcile(files: &dyn AccountFiles) -> Result<(), AppError> {
    let read = |f: AccountFile| -> Result<(String, String), AppError> { Ok((files.read_lower(f)?, files.read_current(f)?)) };
    let (lower_group, current_group) = read(AccountFile::Group)?;
    let (lower_gshadow, current_gshadow) = read(AccountFile::Gshadow)?;
    let (lower_passwd, current_passwd) = read(AccountFile::Passwd)?;
    let (lower_shadow, current_shadow) = read(AccountFile::Shadow)?;
    let plan = plan_reconcile(&ReconcileInput {
        lower_passwd: &lower_passwd,
        current_passwd: &current_passwd,
        lower_shadow: &lower_shadow,
        current_shadow: &current_shadow,
        lower_group: &lower_group,
        current_group: &current_group,
        lower_gshadow: &lower_gshadow,
        current_gshadow: &current_gshadow,
    });
    for name in &plan.conflicts {
        warn!("provision: system entry {name} from the image conflicts with an existing id and was not added");
    }
    // `ALL` order puts groups before the passwd lines that name them.
    for file in AccountFile::ALL {
        if let Some((_, lines)) = plan.appends.iter().find(|(f, _)| *f == file) {
            files.append_lines(file, lines)?;
        }
    }

    let upper = files.list_upper()?;
    let odd = unexpected_upper_entries(upper.iter().map(String::as_str));
    if !odd.is_empty() {
        warn!("provision: unexpected entries in the /etc overlay upper layer: {}", odd.join(", "));
    }

    // Key cleanup trusts passwd, so it only runs against a passwd that
    // plainly is the real one.
    let passwd_now = files.read_current(AccountFile::Passwd)?;
    if passwd_now.lines().any(|l| l.starts_with("root:")) {
        let keys = files.list_key_files()?;
        for name in stale_key_files(keys.iter().map(String::as_str), &passwd_now) {
            files.delete_key_file(&name)?;
            info!("provision: removed the key file of deleted user {name}");
        }
    } else {
        warn!("provision: passwd has no root entry, skipping key file cleanup");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use controller_agent::domain::accounts::password::Secret;
    use controller_agent::domain::accounts::state::AccountState;
    use controller_agent::ports::AccountsError;
    use controller_agent::services::users::testing::FakeEntropy;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    type Log = Mutex<Vec<String>>;

    struct FailingEntropy;

    impl Entropy for FailingEntropy {
        fn fill(&self, _: &mut [u8]) -> Result<(), AppError> {
            Err(AppError::failed("no entropy"))
        }
    }

    struct FakeState {
        held: Mutex<Option<AccountState>>,
        log: &'static Log,
    }

    impl AccountStateStore for FakeState {
        fn load(&self) -> Result<Option<AccountState>, AppError> {
            Ok(self.held.lock().unwrap().clone())
        }

        fn save(&self, state: &AccountState) -> Result<(), AppError> {
            self.log.lock().unwrap().push("state.save".into());
            *self.held.lock().unwrap() = Some(state.clone());
            Ok(())
        }
    }

    /// Records calls; provision never sets a password.
    struct FakeAccounts {
        log: &'static Log,
    }

    impl AccountsPort for FakeAccounts {
        fn set_password(&self, _: &str, _: &Secret) -> Result<(), AccountsError> {
            unreachable!("provision never generates a password")
        }

        fn clear_password(&self, user: &str) -> Result<(), AccountsError> {
            self.log.lock().unwrap().push(format!("accounts.clear_password {user}"));
            Ok(())
        }

        fn set_locked(&self, _: &str, _: bool) -> Result<(), AccountsError> {
            unreachable!("provision never locks")
        }

        fn end_sessions(&self, _: &str) -> Result<(), AccountsError> {
            unreachable!("provision never ends sessions")
        }
    }

    struct FakeFiles {
        available: bool,
        lower: BTreeMap<AccountFile, String>,
        current: Mutex<BTreeMap<AccountFile, String>>,
        upper: Vec<String>,
        keys: Mutex<Vec<String>>,
    }

    impl FakeFiles {
        fn healthy() -> FakeFiles {
            let lower: BTreeMap<_, _> = [
                (AccountFile::Passwd, "root:x:0:0::/root:/bin/sh\nnewsvc:x:995:995::/:/bin/false\nkiosk:x:1000:1000::/var/home/kiosk:/usr/bin/bash\n"),
                (AccountFile::Shadow, "root:*:1::::::\nnewsvc:*:1::::::\nkiosk:!:1::::::\n"),
                (AccountFile::Group, "root:x:0:\nnewsvc:x:995:\nkiosk:x:1000:\n"),
                (AccountFile::Gshadow, "root:*::\nnewsvc:*::\nkiosk:!::\n"),
            ]
            .into_iter()
            .map(|(f, t)| (f, t.to_string()))
            .collect();
            let mut current = lower.clone();
            // The overlay's copy predates `newsvc` and has a managed user.
            current.insert(AccountFile::Passwd, "root:x:0:0::/root:/bin/sh\nkiosk:x:1000:1000::/var/home/kiosk:/usr/bin/bash\nalice:x:1001:1001::/h:/bin/sh\n".into());
            current.insert(AccountFile::Shadow, "root:*:1::::::\nkiosk:!:1::::::\nalice:$6$s$h:1::::::\n".into());
            current.insert(AccountFile::Group, "root:x:0:\nkiosk:x:1000:\nalice:x:1001:\n".into());
            current.insert(AccountFile::Gshadow, "root:*::\nkiosk:!::\nalice:!::\n".into());
            FakeFiles { available: true, lower, current: Mutex::new(current), upper: vec!["passwd".into(), ".pwd.lock".into()], keys: Mutex::new(vec!["alice".into(), "bob".into()]) }
        }
    }

    impl AccountFiles for FakeFiles {
        fn storage_available(&self) -> bool {
            self.available
        }

        fn read_lower(&self, file: AccountFile) -> Result<String, AppError> {
            Ok(self.lower.get(&file).cloned().unwrap_or_default())
        }

        fn read_current(&self, file: AccountFile) -> Result<String, AppError> {
            Ok(self.current.lock().unwrap().get(&file).cloned().unwrap_or_default())
        }

        fn append_lines(&self, file: AccountFile, lines: &[String]) -> Result<(), AppError> {
            let mut cur = self.current.lock().unwrap();
            let text = cur.entry(file).or_default();
            for l in lines {
                text.push_str(l);
                text.push('\n');
            }
            Ok(())
        }

        fn list_upper(&self) -> Result<Vec<String>, AppError> {
            Ok(self.upper.clone())
        }

        fn list_key_files(&self) -> Result<Vec<String>, AppError> {
            Ok(self.keys.lock().unwrap().clone())
        }

        fn delete_key_file(&self, name: &str) -> Result<(), AppError> {
            self.keys.lock().unwrap().retain(|k| k != name);
            Ok(())
        }
    }

    struct World {
        entropy: FakeEntropy,
        state: FakeState,
        accounts: FakeAccounts,
        files: FakeFiles,
        log: &'static Log,
    }

    impl World {
        fn new() -> World {
            // Leaked: the fakes hold a shared event log for ordering asserts.
            let log: &'static Log = Box::leak(Box::new(Mutex::new(Vec::new())));
            World {
                entropy: FakeEntropy::new(42),
                state: FakeState { held: Mutex::new(None), log },
                accounts: FakeAccounts { log },
                files: FakeFiles::healthy(),
                log,
            }
        }

        fn kiosk_hash(self, hash: &str) -> World {
            self.files.current.lock().unwrap().insert(AccountFile::Shadow, format!("root:*:1::::::\nkiosk:{hash}:1::::::\nalice:$6$s$h:1::::::\n"));
            self
        }

        fn run(&self) -> Result<Outcome, AppError> {
            run(&Ports { entropy: &self.entropy, state: &self.state, accounts: &self.accounts, files: &self.files })
        }

        fn events(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn state(&self) -> Option<AccountState> {
            self.state.held.lock().unwrap().clone()
        }
    }

    fn st(ack: bool) -> AccountState {
        AccountState { epoch: "e".repeat(32), kiosk_password_set: true, acknowledged: ack }
    }

    #[test]
    fn a_locked_kiosk_with_no_state_is_left_for_the_prompt_and_nothing_is_written() {
        let w = World::new();
        assert_eq!(w.run().unwrap(), Outcome::Prompt { relocked: false });
        assert!(w.events().is_empty());
        assert!(w.state().is_none());
    }

    #[test]
    fn a_usable_hash_without_state_is_adopted_as_acknowledged() {
        let w = World::new().kiosk_hash("$6$by$hand");
        assert_eq!(w.run().unwrap(), Outcome::Adopted);
        let s = w.state().unwrap();
        assert!(s.acknowledged && s.epoch.len() == 32 && s.epoch.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(w.events(), vec!["state.save"]);
    }

    #[test]
    fn an_acknowledged_usable_hash_is_steady_and_untouched() {
        let w = World::new().kiosk_hash("$6$by$hand");
        *w.state.held.lock().unwrap() = Some(st(true));
        assert_eq!(w.run().unwrap(), Outcome::Steady);
        assert!(w.events().is_empty());
    }

    #[test]
    fn our_unacknowledged_password_is_relocked_and_the_state_stays_unacknowledged() {
        let w = World::new().kiosk_hash("$6$ours$unseen");
        *w.state.held.lock().unwrap() = Some(st(false));
        assert_eq!(w.run().unwrap(), Outcome::Prompt { relocked: true });
        assert_eq!(w.events(), vec!["accounts.clear_password kiosk"]);
        assert!(!w.state().unwrap().acknowledged);
    }

    #[test]
    fn a_locked_hash_with_unacknowledged_state_just_prompts() {
        let w = World::new();
        *w.state.held.lock().unwrap() = Some(st(false));
        assert_eq!(w.run().unwrap(), Outcome::Prompt { relocked: false });
        assert!(w.events().is_empty());
    }

    #[test]
    fn an_entropy_failure_while_adopting_changes_nothing() {
        let w = World::new().kiosk_hash("$6$by$hand");
        let r = run(&Ports { entropy: &FailingEntropy, state: &w.state, accounts: &w.accounts, files: &w.files });
        assert!(r.is_err());
        assert!(w.events().is_empty());
        assert!(w.state().is_none());
    }

    #[test]
    fn degraded_mode_changes_nothing() {
        let mut w = World::new().kiosk_hash("$6$by$hand");
        w.files.available = false;
        assert_eq!(w.run().unwrap(), Outcome::StorageUnavailable);
        assert!(w.events().is_empty());
        assert_eq!(w.files.keys.lock().unwrap().len(), 2, "key files untouched without the overlay");
    }

    #[test]
    fn reconcile_adds_system_entries_and_drops_keys_of_missing_users() {
        let w = World::new();
        w.run().unwrap();
        let cur = w.files.current.lock().unwrap();
        assert!(cur[&AccountFile::Passwd].contains("newsvc:x:995:995::/:/bin/false\n"));
        assert!(cur[&AccountFile::Shadow].contains("newsvc:*:1::::::\n"));
        assert!(cur[&AccountFile::Group].contains("newsvc:x:995:\n"));
        assert!(cur[&AccountFile::Gshadow].contains("newsvc:*::\n"));
        // The managed user and its entry stay untouched.
        assert!(cur[&AccountFile::Passwd].contains("alice:x:1001"));
        drop(cur);
        assert_eq!(*w.files.keys.lock().unwrap(), vec!["alice".to_string()]);
    }

    #[test]
    fn reconcile_runs_on_every_boot_even_when_steady_and_is_idempotent() {
        let w = World::new().kiosk_hash("$6$by$hand");
        *w.state.held.lock().unwrap() = Some(st(true));
        w.run().unwrap();
        let after_first = w.files.current.lock().unwrap().clone();
        w.run().unwrap();
        assert_eq!(*w.files.current.lock().unwrap(), after_first);
    }

    #[test]
    fn key_cleanup_is_skipped_when_passwd_has_no_root() {
        // Neither the overlay copy nor the image has a root entry, so nothing
        // can vouch for this passwd.
        let mut w = World::new();
        w.files.lower.insert(AccountFile::Passwd, "alice:x:1001:1001::/h:/bin/sh\n".into());
        w.files.current.lock().unwrap().insert(AccountFile::Passwd, "alice:x:1001:1001::/h:/bin/sh\n".into());
        *w.state.held.lock().unwrap() = Some(st(true));
        w.run().unwrap();
        assert_eq!(w.files.keys.lock().unwrap().len(), 2);
    }
}
