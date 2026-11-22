//! First-boot state: the persisted account state, the provision planner, the
//! reveal/acknowledge state machine and the boot-time reconcile planner.
//! Pure: time arrives as plain monotonic seconds, files as strings.

use super::password::Secret;
use std::collections::BTreeSet;
use std::fmt;

/// Seconds a generated first-boot password stays revealable.
pub const DEFAULT_TTL_SECS: u64 = 900;
/// The account that receives the first-boot password.
pub const KIOSK_USER: &str = "kiosk";
/// Accounts and groups with an id below this belong to the image.
pub const SYSTEM_ID_LIMIT: u32 = 1000;

/// `^[a-z_][a-z0-9_-]{0,30}$`, checked by hand (no regex dependency).
pub fn is_safe_account_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(b'a'..=b'z') | Some(b'_') => {}
        _ => return false,
    }
    name.len() <= 31 && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

/// `state.json`: what survives a reboot. Never holds a password or a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountState {
    /// Random 128 bit identifier (32 hex characters) written when
    /// provisioning first completes.
    pub epoch: String,
    pub kiosk_password_set: bool,
    pub acknowledged: bool,
}

impl AccountState {
    /// State right after a password was set and before anyone acknowledged it.
    pub fn pending(epoch: String) -> AccountState {
        AccountState { epoch, kiosk_password_set: true, acknowledged: false }
    }

    pub fn acknowledged(&self) -> AccountState {
        AccountState { acknowledged: true, ..self.clone() }
    }
}

/// What the tmpfs holder looks like to `provision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSlot {
    Absent,
    /// Present and not past its expiry.
    Live,
    /// Present but past its expiry: it can no longer be revealed.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionAction {
    /// Acknowledged: nothing to do.
    Steady,
    /// A live, unacknowledged password already exists (a repeat run in the
    /// same boot): leave it alone.
    KeepPending,
    /// Make a new password: first boot, a reboot before acknowledgement, a
    /// crash between the tmpfs write and the state write, or an expired one.
    Generate,
}

/// Plan section 5 steps 2 to 4. Each outcome is safe to repeat.
pub fn plan_provision(state: Option<&AccountState>, slot: SecretSlot) -> ProvisionAction {
    match state {
        Some(s) if s.acknowledged => ProvisionAction::Steady,
        Some(s) if s.kiosk_password_set && slot == SecretSlot::Live => ProvisionAction::KeepPending,
        _ => ProvisionAction::Generate,
    }
}

/// Where the first-boot screen is. `Pending` and `Revealed` are live;
/// `Acknowledged` and `Expired` are final for this boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Pending,
    Revealed,
    Acknowledged,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapError {
    /// The last four characters were wrong; the secret is kept.
    BadAck,
    /// The password timed out and was scrubbed.
    Expired,
    /// Nothing to reveal (already acknowledged).
    NotPending,
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BootstrapError::BadAck => "bad_ack",
            BootstrapError::Expired => "expired",
            BootstrapError::NotPending => "not_pending",
        })
    }
}

impl std::error::Error for BootstrapError {}

/// The one-time password held in memory while the screen is up. The caller
/// gates `reveal` on a fresh knob push; this type only knows about time and
/// the acknowledgement proof.
pub struct FirstBoot {
    phase: Phase,
    password: Option<Secret>,
    deadline_secs: u64,
}

impl FirstBoot {
    /// A password that was just generated (or reloaded from tmpfs) with an
    /// absolute monotonic deadline.
    pub fn pending(password: Secret, deadline_secs: u64) -> FirstBoot {
        FirstBoot { phase: Phase::Pending, password: Some(password), deadline_secs }
    }

    /// The state when there is no secret to show (acknowledged earlier, or
    /// nothing was ever generated this boot).
    pub fn acknowledged() -> FirstBoot {
        FirstBoot { phase: Phase::Acknowledged, password: None, deadline_secs: 0 }
    }

    pub fn deadline_secs(&self) -> u64 {
        self.deadline_secs
    }

    /// Current phase; applies expiry first (scrubbing the password).
    pub fn phase(&mut self, now: u64) -> Phase {
        self.expire_if_due(now);
        self.phase
    }

    /// Seconds left while live, else `None`.
    pub fn expires_in_secs(&mut self, now: u64) -> Option<u64> {
        match self.phase(now) {
            Phase::Pending | Phase::Revealed => Some(self.deadline_secs.saturating_sub(now)),
            _ => None,
        }
    }

    /// Hands out the password and moves to `Revealed`. Repeatable until
    /// acknowledged or expired, so a Chromium crash does not lose it.
    pub fn reveal(&mut self, now: u64) -> Result<&Secret, BootstrapError> {
        self.expire_if_due(now);
        match self.phase {
            Phase::Expired => Err(BootstrapError::Expired),
            Phase::Acknowledged => Err(BootstrapError::NotPending),
            Phase::Pending | Phase::Revealed => {
                self.phase = Phase::Revealed;
                self.password.as_ref().ok_or(BootstrapError::NotPending)
            }
        }
    }

    /// Acknowledgement by proof of the last four characters. A wrong value
    /// changes nothing. Acknowledging twice is fine.
    pub fn acknowledge(&mut self, now: u64, last4: &str) -> Result<(), BootstrapError> {
        self.expire_if_due(now);
        match self.phase {
            Phase::Expired => Err(BootstrapError::Expired),
            Phase::Acknowledged => Ok(()),
            Phase::Pending | Phase::Revealed => {
                let ok = self.password.as_ref().is_some_and(|p| p.last4_matches(last4));
                if !ok {
                    return Err(BootstrapError::BadAck);
                }
                self.finish(Phase::Acknowledged);
                Ok(())
            }
        }
    }

    /// The first successful `sudo` or unlock counts as acknowledgement.
    pub fn acknowledge_by_proof(&mut self) {
        if matches!(self.phase, Phase::Pending | Phase::Revealed) {
            self.finish(Phase::Acknowledged);
        }
    }

    fn expire_if_due(&mut self, now: u64) {
        if matches!(self.phase, Phase::Pending | Phase::Revealed) && now >= self.deadline_secs {
            self.finish(Phase::Expired);
        }
    }

    /// Final phases drop the in-memory copy (which zeroes it).
    fn finish(&mut self, phase: Phase) {
        self.phase = phase;
        self.password = None;
    }
}

impl fmt::Debug for FirstBoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirstBoot")
            .field("phase", &self.phase)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("deadline_secs", &self.deadline_secs)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Reconcile (plan D10): a newer image may add system accounts or groups; the
// overlay's upper copy of the files hides them, so they are appended.
// ---------------------------------------------------------------------------

/// The four account files the reconcile touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccountFile {
    Group,
    Gshadow,
    Passwd,
    Shadow,
}

impl AccountFile {
    /// Order matters: groups before the passwd entries that name them.
    pub const ALL: [AccountFile; 4] = [AccountFile::Group, AccountFile::Gshadow, AccountFile::Passwd, AccountFile::Shadow];

    pub fn file_name(self) -> &'static str {
        match self {
            AccountFile::Group => "group",
            AccountFile::Gshadow => "gshadow",
            AccountFile::Passwd => "passwd",
            AccountFile::Shadow => "shadow",
        }
    }
}

/// Entries in the upper layer that are expected: the four files, their `-`
/// backups, the lock and subid files shadow-utils touches, and `sudoers.d`.
pub const UPPER_WHITELIST: &[&str] = &[
    "passwd", "passwd-", "shadow", "shadow-", "group", "group-", "gshadow", "gshadow-", ".pwd.lock", "subuid", "subuid-", "subgid", "subgid-", "sudoers.d",
];

pub fn unexpected_upper_entries<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    names.into_iter().filter(|n| !UPPER_WHITELIST.contains(n)).map(str::to_string).collect()
}

/// A line of one of the four files, reduced to its name and numeric id.
fn field(line: &str, idx: usize) -> Option<&str> {
    line.split(':').nth(idx)
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
}

fn names_of(text: &str) -> BTreeSet<&str> {
    lines(text).filter_map(|l| field(l, 0)).collect()
}

fn ids_of(text: &str) -> BTreeSet<u32> {
    lines(text).filter_map(|l| field(l, 2)).filter_map(|v| v.parse().ok()).collect()
}

fn is_system_line(line: &str) -> bool {
    field(line, 2).and_then(|v| v.parse::<u32>().ok()).is_some_and(|id| id < SYSTEM_ID_LIMIT)
}

/// The text of one file as read from the lower layer and from the merged
/// `/etc` (which, once the overlay is up, is the upper copy).
pub struct ReconcileInput<'a> {
    pub lower_passwd: &'a str,
    pub current_passwd: &'a str,
    pub lower_shadow: &'a str,
    pub current_shadow: &'a str,
    pub lower_group: &'a str,
    pub current_group: &'a str,
    pub lower_gshadow: &'a str,
    pub current_gshadow: &'a str,
}

/// Lines to append per file, plus names that could not be appended because
/// their id is already taken by another name (logged by name only).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    pub appends: Vec<(AccountFile, Vec<String>)>,
    pub conflicts: Vec<String>,
}

impl ReconcilePlan {
    pub fn is_empty(&self) -> bool {
        self.appends.is_empty()
    }
}

/// System accounts (uid below 1000) and groups (gid below 1000) present in
/// the lower files and missing, by name, from the current ones. `shadow` and
/// `gshadow` follow by name: the entries for the system names of the lower
/// passwd and group that the current files lack, so an interrupted earlier
/// run is completed rather than skipped.
pub fn plan_reconcile(i: &ReconcileInput<'_>) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    let mut push = |file: AccountFile, v: Vec<String>| {
        if !v.is_empty() {
            plan.appends.push((file, v));
        }
    };

    let cur_group_names = names_of(i.current_group);
    let cur_gids = ids_of(i.current_group);
    let mut group_add = Vec::new();
    let mut group_conflicts: Vec<String> = Vec::new();
    for l in lines(i.lower_group).filter(|l| is_system_line(l)) {
        let name = field(l, 0).unwrap_or_default();
        if cur_group_names.contains(name) {
            continue;
        }
        let gid: u32 = field(l, 2).and_then(|v| v.parse().ok()).unwrap_or(0);
        if cur_gids.contains(&gid) {
            group_conflicts.push(name.to_string());
        } else {
            group_add.push(l.to_string());
        }
    }
    let lower_system_groups: BTreeSet<&str> = lines(i.lower_group).filter(|l| is_system_line(l)).filter_map(|l| field(l, 0)).collect();
    let cur_gshadow_names = names_of(i.current_gshadow);
    let gshadow_add: Vec<String> = lines(i.lower_gshadow)
        .filter(|l| field(l, 0).is_some_and(|n| lower_system_groups.contains(n) && !cur_gshadow_names.contains(n)))
        .filter(|l| field(l, 0).is_some_and(|n| !group_conflicts.iter().any(|c| c == n)))
        .map(str::to_string)
        .collect();

    let cur_user_names = names_of(i.current_passwd);
    let cur_uids = ids_of(i.current_passwd);
    let mut passwd_add = Vec::new();
    let mut user_conflicts: Vec<String> = Vec::new();
    for l in lines(i.lower_passwd).filter(|l| is_system_line(l)) {
        let name = field(l, 0).unwrap_or_default();
        if cur_user_names.contains(name) {
            continue;
        }
        let uid: u32 = field(l, 2).and_then(|v| v.parse().ok()).unwrap_or(0);
        if cur_uids.contains(&uid) {
            user_conflicts.push(name.to_string());
        } else {
            passwd_add.push(l.to_string());
        }
    }
    let lower_system_users: BTreeSet<&str> = lines(i.lower_passwd).filter(|l| is_system_line(l)).filter_map(|l| field(l, 0)).collect();
    let cur_shadow_names = names_of(i.current_shadow);
    let shadow_add: Vec<String> = lines(i.lower_shadow)
        .filter(|l| field(l, 0).is_some_and(|n| lower_system_users.contains(n) && !cur_shadow_names.contains(n)))
        .filter(|l| field(l, 0).is_some_and(|n| !user_conflicts.iter().any(|c| c == n)))
        .map(str::to_string)
        .collect();

    push(AccountFile::Group, group_add);
    push(AccountFile::Gshadow, gshadow_add);
    push(AccountFile::Passwd, passwd_add);
    push(AccountFile::Shadow, shadow_add);
    plan.conflicts = group_conflicts.into_iter().chain(user_conflicts).collect();
    plan
}

/// Key files in the store whose owner no longer exists in `passwd`.
/// Names that are not valid account names are never returned: a stray file
/// is not guessed to be a key.
pub fn stale_key_files<'a>(key_file_names: impl IntoIterator<Item = &'a str>, current_passwd: &str) -> Vec<String> {
    let users = names_of(current_passwd);
    key_file_names.into_iter().filter(|n| is_safe_account_name(n) && !users.contains(n)).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(set: bool, ack: bool) -> AccountState {
        AccountState { epoch: "e".repeat(32), kiosk_password_set: set, acknowledged: ack }
    }

    #[test]
    fn names() {
        for ok in ["kiosk", "_x", "a-b_c9", &"a".repeat(31)] {
            assert!(is_safe_account_name(ok), "{ok}");
        }
        for bad in ["", "Kiosk", "1a", "a b", "a:b", "-a", "a\n", &"a".repeat(32), "é"] {
            assert!(!is_safe_account_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn provision_plan_covers_every_combination() {
        use ProvisionAction::*;
        use SecretSlot::*;
        // No state: first boot (a leftover live secret means a crash between
        // the tmpfs write and the state write, so regenerate).
        assert_eq!(plan_provision(None, Absent), Generate);
        assert_eq!(plan_provision(None, Live), Generate);
        assert_eq!(plan_provision(None, Expired), Generate);
        // Acknowledged: steady whatever the tmpfs says.
        for slot in [Absent, Live, Expired] {
            assert_eq!(plan_provision(Some(&st(true, true)), slot), Steady);
        }
        // Reboot before acknowledgement: tmpfs is empty, regenerate.
        assert_eq!(plan_provision(Some(&st(true, false)), Absent), Generate);
        // Repeat run in the same boot keeps the live password.
        assert_eq!(plan_provision(Some(&st(true, false)), Live), KeepPending);
        assert_eq!(plan_provision(Some(&st(true, false)), Expired), Generate);
        // State written but no password recorded as set.
        assert_eq!(plan_provision(Some(&st(false, false)), Live), Generate);
        assert_eq!(plan_provision(Some(&st(false, false)), Absent), Generate);
    }

    fn fb(deadline: u64) -> FirstBoot {
        FirstBoot::pending(Secret::new("Abcdefgh12345678"), deadline)
    }

    #[test]
    fn pending_to_revealed_and_repeatable() {
        let mut f = fb(900);
        assert_eq!(f.phase(0), Phase::Pending);
        assert_eq!(f.reveal(1).unwrap().expose(), "Abcdefgh12345678");
        assert_eq!(f.phase(2), Phase::Revealed);
        // A second reveal (Chromium crashed) still works.
        assert_eq!(f.reveal(3).unwrap().expose(), "Abcdefgh12345678");
        assert_eq!(f.expires_in_secs(100), Some(800));
    }

    #[test]
    fn wrong_last4_keeps_the_secret_and_the_phase() {
        let mut f = fb(900);
        f.reveal(1).unwrap();
        assert_eq!(f.acknowledge(2, "0000"), Err(BootstrapError::BadAck));
        assert_eq!(f.acknowledge(2, "567"), Err(BootstrapError::BadAck));
        assert_eq!(f.phase(2), Phase::Revealed);
        assert!(f.reveal(3).is_ok());
    }

    #[test]
    fn right_last4_acknowledges_and_drops_the_secret() {
        let mut f = fb(900);
        f.reveal(1).unwrap();
        assert_eq!(f.acknowledge(2, "5678"), Ok(()));
        assert_eq!(f.phase(3), Phase::Acknowledged);
        assert_eq!(f.reveal(4).err(), Some(BootstrapError::NotPending));
        assert_eq!(f.expires_in_secs(4), None);
        // Idempotent, whatever the second proof says.
        assert_eq!(f.acknowledge(5, "zzzz"), Ok(()));
    }

    #[test]
    fn ack_from_pending_is_allowed_with_proof() {
        let mut f = fb(900);
        assert_eq!(f.acknowledge(1, "5678"), Ok(()));
        assert_eq!(f.phase(1), Phase::Acknowledged);
    }

    #[test]
    fn expiry_scrubs_and_blocks_reveal_and_ack() {
        let mut f = fb(900);
        f.reveal(1).unwrap();
        assert_eq!(f.expires_in_secs(899), Some(1));
        assert_eq!(f.phase(900), Phase::Expired);
        assert_eq!(f.reveal(901).err(), Some(BootstrapError::Expired));
        assert_eq!(f.acknowledge(901, "5678"), Err(BootstrapError::Expired));
        assert_eq!(f.expires_in_secs(901), None);
    }

    #[test]
    fn expiry_applies_on_the_call_that_notices_it() {
        let mut f = fb(10);
        assert_eq!(f.reveal(10).err(), Some(BootstrapError::Expired));
        let mut g = fb(10);
        assert_eq!(g.acknowledge(11, "5678"), Err(BootstrapError::Expired));
    }

    #[test]
    fn proof_of_knowledge_acknowledges_without_last4() {
        let mut f = fb(900);
        f.acknowledge_by_proof();
        assert_eq!(f.phase(1), Phase::Acknowledged);
        let mut g = fb(1);
        assert_eq!(g.phase(5), Phase::Expired);
        g.acknowledge_by_proof();
        assert_eq!(g.phase(5), Phase::Expired);
    }

    #[test]
    fn steady_state_has_nothing_to_reveal() {
        let mut f = FirstBoot::acknowledged();
        assert_eq!(f.phase(0), Phase::Acknowledged);
        assert_eq!(f.reveal(0).err(), Some(BootstrapError::NotPending));
    }

    #[test]
    fn first_boot_debug_never_prints_the_password() {
        let f = fb(900);
        let shown = format!("{f:?}");
        assert!(!shown.contains("Abcdefgh"), "{shown}");
        assert!(shown.contains("redacted"));
    }

    const PASSWD_LOWER: &str = "root:x:0:0:root:/root:/bin/sh\ndaemon:x:1:1:daemon:/usr/sbin:/bin/sh\nnewsvc:x:995:995::/:/bin/false\nkiosk:x:1000:1000::/var/home/kiosk:/usr/bin/bash\nnobody:x:65534:65534::/:/bin/false\n";
    const SHADOW_LOWER: &str = "root:*:1::::::\ndaemon:*:1::::::\nnewsvc:*:1::::::\nkiosk:!:1::::::\n";
    const GROUP_LOWER: &str = "root:x:0:\nsudo:x:27:kiosk\nnewsvc:x:995:\nkiosk:x:1000:\n";
    const GSHADOW_LOWER: &str = "root:*::\nsudo:*::kiosk\nnewsvc:*::\nkiosk:!::\n";

    #[test]
    fn reconcile_appends_only_missing_system_entries() {
        let cur_passwd = "root:x:0:0:root:/root:/bin/sh\ndaemon:x:1:1:daemon:/usr/sbin:/bin/sh\nkiosk:x:1000:1000::/var/home/kiosk:/usr/bin/bash\nalice:x:1001:1001::/var/home/alice:/usr/bin/bash\n";
        let cur_shadow = "root:*:1::::::\ndaemon:*:1::::::\nkiosk:$6$x$y:1::::::\nalice:$6$x$y:1::::::\n";
        let cur_group = "root:x:0:\nsudo:x:27:kiosk,alice\nkiosk:x:1000:\nalice:x:1001:\n";
        let cur_gshadow = "root:*::\nsudo:*::kiosk,alice\nkiosk:!::\nalice:!::\n";
        let plan = plan_reconcile(&ReconcileInput {
            lower_passwd: PASSWD_LOWER,
            current_passwd: cur_passwd,
            lower_shadow: SHADOW_LOWER,
            current_shadow: cur_shadow,
            lower_group: GROUP_LOWER,
            current_group: cur_group,
            lower_gshadow: GSHADOW_LOWER,
            current_gshadow: cur_gshadow,
        });
        assert_eq!(
            plan.appends,
            vec![
                (AccountFile::Group, vec!["newsvc:x:995:".to_string()]),
                (AccountFile::Gshadow, vec!["newsvc:*::".to_string()]),
                (AccountFile::Passwd, vec!["newsvc:x:995:995::/:/bin/false".to_string()]),
                (AccountFile::Shadow, vec!["newsvc:*:1::::::".to_string()]),
            ]
        );
        assert!(plan.conflicts.is_empty());
    }

    #[test]
    fn reconcile_is_idempotent_and_never_touches_managed_or_changed_entries() {
        let plan = plan_reconcile(&ReconcileInput {
            lower_passwd: PASSWD_LOWER,
            current_passwd: PASSWD_LOWER,
            lower_shadow: SHADOW_LOWER,
            current_shadow: SHADOW_LOWER,
            lower_group: GROUP_LOWER,
            current_group: GROUP_LOWER,
            lower_gshadow: GSHADOW_LOWER,
            current_gshadow: GSHADOW_LOWER,
        });
        assert!(plan.is_empty());
        // A system account that exists with a different shell or hash is not
        // rewritten: only names that are absent are added.
        let changed = "root:x:0:0:root:/root:/bin/false\n";
        let p2 = plan_reconcile(&ReconcileInput {
            lower_passwd: "root:x:0:0:root:/root:/bin/sh\n",
            current_passwd: changed,
            lower_shadow: "root:*:1::::::\n",
            current_shadow: "root:!:1::::::\n",
            lower_group: "root:x:0:\n",
            current_group: "root:x:0:\n",
            lower_gshadow: "",
            current_gshadow: "",
        });
        assert!(p2.is_empty());
    }

    #[test]
    fn reconcile_completes_an_interrupted_run() {
        // passwd got the account but the crash came before shadow.
        let plan = plan_reconcile(&ReconcileInput {
            lower_passwd: PASSWD_LOWER,
            current_passwd: PASSWD_LOWER,
            lower_shadow: SHADOW_LOWER,
            current_shadow: "root:*:1::::::\ndaemon:*:1::::::\nkiosk:!:1::::::\n",
            lower_group: GROUP_LOWER,
            current_group: GROUP_LOWER,
            lower_gshadow: GSHADOW_LOWER,
            current_gshadow: GSHADOW_LOWER,
        });
        assert_eq!(plan.appends, vec![(AccountFile::Shadow, vec!["newsvc:*:1::::::".to_string()])]);
    }

    #[test]
    fn reconcile_reports_id_conflicts_by_name_and_does_not_append() {
        let plan = plan_reconcile(&ReconcileInput {
            lower_passwd: "newsvc:x:995:995::/:/bin/false\n",
            current_passwd: "other:x:995:995::/:/bin/false\n",
            lower_shadow: "newsvc:*:1::::::\n",
            current_shadow: "other:*:1::::::\n",
            lower_group: "newsvc:x:995:\n",
            current_group: "othergrp:x:995:\n",
            lower_gshadow: "newsvc:*::\n",
            current_gshadow: "othergrp:*::\n",
        });
        assert!(plan.appends.is_empty(), "{plan:?}");
        assert_eq!(plan.conflicts, vec!["newsvc".to_string(), "newsvc".to_string()]);
    }

    #[test]
    fn reconcile_ignores_comments_and_blank_lines() {
        let plan = plan_reconcile(&ReconcileInput {
            lower_passwd: "# c\n\nroot:x:0:0:root:/root:/bin/sh\n",
            current_passwd: "",
            lower_shadow: "",
            current_shadow: "",
            lower_group: "",
            current_group: "",
            lower_gshadow: "",
            current_gshadow: "",
        });
        assert_eq!(plan.appends, vec![(AccountFile::Passwd, vec!["root:x:0:0:root:/root:/bin/sh".to_string()])]);
    }

    #[test]
    fn stale_keys_are_those_without_a_user() {
        let passwd = "root:x:0:0::/root:/bin/sh\nalice:x:1001:1001::/h:/bin/sh\n";
        let found = stale_key_files(["alice", "bob", ".hidden", "Bad Name", "carol"], passwd);
        assert_eq!(found, vec!["bob".to_string(), "carol".to_string()]);
    }

    #[test]
    fn upper_layer_whitelist() {
        let odd = unexpected_upper_entries(["passwd", "shadow-", ".pwd.lock", "subgid-", "sudoers.d", "hostname", "evil"]);
        assert_eq!(odd, vec!["hostname".to_string(), "evil".to_string()]);
    }
}
