//! `controller-agent provision`: runs once per boot from
//! `thing-revived-provision.service` and is a fast no-op in steady state.
//!
//! 1. Reconcile (plan D10): append system accounts and groups the image has
//!    and the `/etc` overlay's upper copy lacks, delete key files of users
//!    that no longer exist, warn about unexpected upper files, check that the
//!    baked key is in place.
//! 2. First-boot password (plan section 5): unless acknowledged, generate a
//!    16 character password, write it to the tmpfs holder first, set the
//!    kiosk password with it, then record `state.json`.
//!
//! Everything goes through ports, so the tests below run with fakes. The
//! password is never formatted into a log line or an error.

use controller_agent::adapters::accounts_shadow::{EtcFiles, EtcPaths, JsonStateFile, ShadowAccounts, DEFAULT_SHA_ROUNDS};
use controller_agent::adapters::os_entropy::OsEntropy;
use controller_agent::adapters::secret_tmpfs::{ProcUptime, TmpfsSecretStore};
use controller_agent::app::error::AppError;
use controller_agent::domain::accounts::password::{self, GenerateError};
use controller_agent::domain::accounts::state::{
    plan_provision, plan_reconcile, stale_key_files, unexpected_upper_entries, AccountFile, AccountState, ProvisionAction, ReconcileInput, SecretSlot,
    DEFAULT_TTL_SECS, KIOSK_USER,
};
use controller_agent::ports::{AccountFiles, AccountStateStore, AccountsPort, Entropy, MonotonicClock, SecretStore};
use controller_agent::utils::config::{Config, LoggingConfig};
use controller_agent::utils::logging;
use log::{error, info, warn};

/// Plan `[users]` defaults for the two directories this command owns.
const SECRET_DIR: &str = "/run/thing-revived";
const STATE_DIR: &str = "/var/lib/thing-revived/accounts";

pub struct Ports<'a> {
    pub entropy: &'a dyn Entropy,
    pub clock: &'a dyn MonotonicClock,
    pub secrets: &'a dyn SecretStore,
    pub state: &'a dyn AccountStateStore,
    pub accounts: &'a dyn AccountsPort,
    pub files: &'a dyn AccountFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The `/etc` overlay is not up (degraded mode): nothing was changed.
    StorageUnavailable,
    /// Already acknowledged.
    Steady,
    /// A live unacknowledged password from earlier in this boot was kept.
    KeptPending,
    /// A new password was generated and set.
    Generated,
}

/// Process entry for the subcommand; returns the exit status.
pub fn command() -> i32 {
    // A broken controller.toml must not stop account provisioning.
    let logging_cfg = Config::load().map(|l| l.config.logging).unwrap_or_else(|_| LoggingConfig::default());
    logging::init(&logging_cfg);
    let entropy = OsEntropy::new();
    let clock = ProcUptime::new();
    let secrets = TmpfsSecretStore::new(SECRET_DIR);
    let state = JsonStateFile::new(STATE_DIR);
    let accounts = ShadowAccounts::new(DEFAULT_SHA_ROUNDS);
    let files = EtcFiles::new(EtcPaths::default());
    let ports = Ports { entropy: &entropy, clock: &clock, secrets: &secrets, state: &state, accounts: &accounts, files: &files };
    match run(&ports, DEFAULT_TTL_SECS) {
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

pub fn run(p: &Ports<'_>, ttl_secs: u64) -> Result<Outcome, AppError> {
    if !p.files.storage_available() {
        return Ok(Outcome::StorageUnavailable);
    }
    // A reconcile problem is loud but must not block the password: without
    // it the owner has no way to sudo.
    if let Err(e) = reconcile(p.files) {
        error!("provision: reconcile failed: {e}");
    }
    first_boot(p, ttl_secs)
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

    if !files.baked_key_present() {
        warn!("provision: the baked authorized key is missing from the image");
    }
    Ok(())
}

fn first_boot(p: &Ports<'_>, ttl_secs: u64) -> Result<Outcome, AppError> {
    let state = p.state.load()?;
    let now = p.clock.now_secs();
    let slot = match p.secrets.load()? {
        None => SecretSlot::Absent,
        Some(s) if s.expires_at_secs > now => SecretSlot::Live,
        Some(_) => SecretSlot::Expired,
    };
    match plan_provision(state.as_ref(), slot) {
        ProvisionAction::Steady => return Ok(Outcome::Steady),
        ProvisionAction::KeepPending => return Ok(Outcome::KeptPending),
        ProvisionAction::Generate => {}
    }

    let secret = password::generate(|buf| p.entropy.fill(buf)).map_err(|e| match e {
        GenerateError::Entropy(e) => e,
        GenerateError::Exhausted => AppError::failed("entropy source gave no usable bytes"),
    })?;
    let epoch = match &state {
        Some(s) => s.epoch.clone(),
        None => new_epoch(p.entropy)?,
    };

    // Plaintext first: a crash after this line leaves a secret that no hash
    // matches yet, which the next run replaces; the other order could leave a
    // password nobody knows.
    p.secrets.store(&secret, now + ttl_secs)?;
    if let Err(e) = p.accounts.set_password(KIOSK_USER, &secret) {
        let _ = p.secrets.scrub();
        return Err(e.into());
    }
    p.state.save(&AccountState::pending(epoch))?;
    info!("provision: a new {KIOSK_USER} first-boot password was set and is waiting to be revealed");
    Ok(Outcome::Generated)
}

fn new_epoch(entropy: &dyn Entropy) -> Result<String, AppError> {
    let mut bytes = [0u8; 16];
    entropy.fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use controller_agent::domain::accounts::password::Secret;
    use controller_agent::ports::{AccountsError, StoredSecret};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    type Log = Mutex<Vec<String>>;

    struct FakeEntropy(Mutex<u64>);

    impl FakeEntropy {
        fn new(seed: u64) -> FakeEntropy {
            FakeEntropy(Mutex::new(seed))
        }
    }

    impl Entropy for FakeEntropy {
        fn fill(&self, buf: &mut [u8]) -> Result<(), AppError> {
            let mut s = self.0.lock().unwrap();
            for b in buf.iter_mut() {
                *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *b = (*s >> 33) as u8;
            }
            Ok(())
        }
    }

    struct FailingEntropy;

    impl Entropy for FailingEntropy {
        fn fill(&self, _: &mut [u8]) -> Result<(), AppError> {
            Err(AppError::failed("no entropy"))
        }
    }

    struct FakeClock(AtomicU64);

    impl MonotonicClock for FakeClock {
        fn now_secs(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct FakeSecrets {
        held: Mutex<Option<(String, u64)>>,
        log: &'static Log,
    }

    impl SecretStore for FakeSecrets {
        fn store(&self, secret: &Secret, expires_at_secs: u64) -> Result<(), AppError> {
            self.log.lock().unwrap().push("secret.store".into());
            *self.held.lock().unwrap() = Some((secret.expose().to_string(), expires_at_secs));
            Ok(())
        }

        fn load(&self) -> Result<Option<StoredSecret>, AppError> {
            Ok(self.held.lock().unwrap().as_ref().map(|(p, e)| StoredSecret { secret: Secret::new(p.clone()), expires_at_secs: *e }))
        }

        fn scrub(&self) -> Result<(), AppError> {
            self.log.lock().unwrap().push("secret.scrub".into());
            *self.held.lock().unwrap() = None;
            Ok(())
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

    struct FakeAccounts {
        set: Mutex<Vec<(String, String)>>,
        fail: bool,
        log: &'static Log,
    }

    impl AccountsPort for FakeAccounts {
        fn set_password(&self, user: &str, password: &Secret) -> Result<(), AccountsError> {
            self.log.lock().unwrap().push("accounts.set_password".into());
            if self.fail {
                return Err(AccountsError::Tool(AppError::failed("chpasswd exited with status 1")));
            }
            self.set.lock().unwrap().push((user.to_string(), password.expose().to_string()));
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
        baked: bool,
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
            FakeFiles { available: true, lower, current: Mutex::new(current), upper: vec!["passwd".into(), ".pwd.lock".into()], keys: Mutex::new(vec!["alice".into(), "bob".into()]), baked: true }
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

        fn baked_key_present(&self) -> bool {
            self.baked
        }
    }

    struct World {
        entropy: FakeEntropy,
        clock: FakeClock,
        secrets: FakeSecrets,
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
                clock: FakeClock(AtomicU64::new(100)),
                secrets: FakeSecrets { held: Mutex::new(None), log },
                state: FakeState { held: Mutex::new(None), log },
                accounts: FakeAccounts { set: Mutex::new(Vec::new()), fail: false, log },
                files: FakeFiles::healthy(),
                log,
            }
        }

        fn run(&self) -> Result<Outcome, AppError> {
            run(
                &Ports { entropy: &self.entropy, clock: &self.clock, secrets: &self.secrets, state: &self.state, accounts: &self.accounts, files: &self.files },
                900,
            )
        }

        fn events(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn last_password(&self) -> String {
            self.accounts.set.lock().unwrap().last().unwrap().1.clone()
        }
    }

    #[test]
    fn first_boot_generates_stores_sets_then_saves_in_that_order() {
        let w = World::new();
        assert_eq!(w.run().unwrap(), Outcome::Generated);
        assert_eq!(w.events(), vec!["secret.store", "accounts.set_password", "state.save"]);
        let (pw, expires) = w.secrets.held.lock().unwrap().clone().unwrap();
        assert_eq!(pw, w.last_password());
        assert_eq!(pw.len(), 16);
        assert_eq!(expires, 100 + 900);
        assert_eq!(w.accounts.set.lock().unwrap()[0].0, "kiosk");
        let st = w.state.held.lock().unwrap().clone().unwrap();
        assert!(st.kiosk_password_set && !st.acknowledged);
        assert_eq!(st.epoch.len(), 32);
        assert!(st.epoch.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn a_repeat_run_in_the_same_boot_keeps_the_live_password() {
        let w = World::new();
        w.run().unwrap();
        let first = w.last_password();
        assert_eq!(w.run().unwrap(), Outcome::KeptPending);
        assert_eq!(w.accounts.set.lock().unwrap().len(), 1);
        assert_eq!(w.last_password(), first);
    }

    #[test]
    fn a_reboot_before_acknowledgement_regenerates_and_keeps_the_epoch() {
        let w = World::new();
        w.run().unwrap();
        let (first_pw, epoch) = (w.last_password(), w.state.held.lock().unwrap().clone().unwrap().epoch);
        // Reboot: tmpfs is gone, /var state stays.
        *w.secrets.held.lock().unwrap() = None;
        assert_eq!(w.run().unwrap(), Outcome::Generated);
        assert_ne!(w.last_password(), first_pw);
        assert_eq!(w.state.held.lock().unwrap().clone().unwrap().epoch, epoch);
        assert_eq!(w.accounts.set.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_crash_between_the_tmpfs_write_and_the_state_write_regenerates() {
        let w = World::new();
        // Leftover from a run that died after the secret was stored.
        *w.secrets.held.lock().unwrap() = Some(("Leftover12345678".into(), 5000));
        assert_eq!(w.run().unwrap(), Outcome::Generated);
        assert_ne!(w.last_password(), "Leftover12345678");
        assert_eq!(w.secrets.held.lock().unwrap().as_ref().unwrap().0, w.last_password());
    }

    #[test]
    fn an_expired_secret_is_replaced_not_kept() {
        let w = World::new();
        w.run().unwrap();
        w.clock.0.store(100 + 900, Ordering::SeqCst);
        assert_eq!(w.run().unwrap(), Outcome::Generated);
        assert_eq!(w.accounts.set.lock().unwrap().len(), 2);
    }

    #[test]
    fn an_acknowledged_account_is_left_alone() {
        let w = World::new();
        *w.state.held.lock().unwrap() = Some(AccountState { epoch: "e".repeat(32), kiosk_password_set: true, acknowledged: true });
        assert_eq!(w.run().unwrap(), Outcome::Steady);
        assert!(w.accounts.set.lock().unwrap().is_empty());
        assert!(w.events().is_empty());
    }

    #[test]
    fn a_failed_password_set_scrubs_the_secret_and_writes_no_state() {
        let mut w = World::new();
        w.accounts.fail = true;
        let err = w.run().unwrap_err();
        assert!(err.message.contains("chpasswd"), "{err}");
        assert!(w.secrets.held.lock().unwrap().is_none());
        assert!(w.state.held.lock().unwrap().is_none());
        assert_eq!(w.events(), vec!["secret.store", "accounts.set_password", "secret.scrub"]);
    }

    #[test]
    fn an_entropy_failure_changes_nothing() {
        let w = World::new();
        let r = run(&Ports { entropy: &FailingEntropy, clock: &w.clock, secrets: &w.secrets, state: &w.state, accounts: &w.accounts, files: &w.files }, 900);
        assert!(r.is_err());
        assert!(w.events().is_empty());
    }

    #[test]
    fn degraded_mode_changes_nothing() {
        let mut w = World::new();
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
        let w = World::new();
        *w.state.held.lock().unwrap() = Some(AccountState { epoch: "e".repeat(32), kiosk_password_set: true, acknowledged: true });
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
        *w.state.held.lock().unwrap() = Some(AccountState { epoch: "e".repeat(32), kiosk_password_set: true, acknowledged: true });
        w.run().unwrap();
        assert_eq!(w.files.keys.lock().unwrap().len(), 2);
    }

    // Captures every log record of this test process.
    struct Capture;
    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, r: &log::Record) {
            CAPTURED.lock().unwrap().push(format!("{}", r.args()));
        }

        fn flush(&self) {}
    }

    #[test]
    fn the_password_never_reaches_a_log_line_or_an_error() {
        let _ = log::set_logger(&Capture);
        log::set_max_level(log::LevelFilter::Trace);
        let mut w = World::new();
        w.run().unwrap();
        let pw = w.last_password();
        w.accounts.fail = true;
        *w.secrets.held.lock().unwrap() = None;
        let err = w.run().unwrap_err();
        let mut seen = vec![format!("{err}"), format!("{err:?}")];
        seen.extend(CAPTURED.lock().unwrap().iter().cloned());
        assert!(!CAPTURED.lock().unwrap().is_empty(), "logger was not installed, test would be vacuous");
        for line in seen {
            assert!(!line.contains(&pw), "password leaked: {line}");
            assert!(!line.contains(&w.last_password()), "password leaked: {line}");
        }
    }
}
