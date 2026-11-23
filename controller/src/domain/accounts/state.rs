//! First-boot state: the persisted account state, the boot decision table
//! (delta plan section 4.1), the phase machine (section 4.2) and the boot-time
//! reconcile planner. Pure: time arrives as `Instant` values, files as strings.
//!
//! Invariant I1 (the ordering rule the services must keep): the unacknowledged
//! `state.json` is written BEFORE the password hash is set. A power cut between
//! the two writes then leaves "usable hash, unacknowledged state", which
//! `decide_boot` answers with a re-lock and the prompt, never with an adopted
//! password nobody saw.

use super::password::Secret;
use std::collections::BTreeSet;
use std::fmt;
use std::time::{Duration, Instant};

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
    /// Random 128 bit identifier (32 hex characters) written when the state
    /// file is first created.
    pub epoch: String,
    /// A password generation was started (set together with the
    /// unacknowledged write that precedes `chpasswd`, invariant I1).
    pub kiosk_password_set: bool,
    pub acknowledged: bool,
}

impl AccountState {
    /// State written before `chpasswd`: a password is about to exist and
    /// nobody has seen it.
    pub fn pending(epoch: String) -> AccountState {
        AccountState { epoch, kiosk_password_set: true, acknowledged: false }
    }

    pub fn acknowledged(&self) -> AccountState {
        AccountState { acknowledged: true, ..self.clone() }
    }
}

/// The hash field of `user`'s row in `shadow` text, if the row exists.
pub fn shadow_hash<'a>(shadow: &'a str, user: &str) -> Option<&'a str> {
    shadow.lines().find_map(|l| {
        let mut f = l.split(':');
        (f.next()? == user).then(|| f.next()).flatten()
    })
}

/// A hash that can authenticate once unlocked: after the lock prefix `!` it is
/// neither empty nor the `*` placeholder. So `!` and `*` and empty are unusable,
/// while `!$6$...` (a real hash an admin locked) still counts as usable.
pub fn hash_is_usable(hash: &str) -> bool {
    let rest = hash.trim_start_matches('!');
    !rest.is_empty() && rest != "*"
}

/// Delta plan 4.1 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDecision {
    /// Usable hash, no state file: a hash set by hand or a restored `/var`.
    /// Write `acknowledged=true`; never generate.
    Adopt,
    /// Usable hash, acknowledged: nothing to do, nothing to show.
    Steady,
    /// Usable hash and unacknowledged state: our own password that nobody
    /// confirmed. Re-lock the hash to `!`, keep the state, show the prompt.
    RelockThenPrompt,
    /// No usable hash: nothing generated yet. Show the prompt.
    Prompt,
}

pub fn decide_boot(hash_usable: bool, state: Option<&AccountState>) -> BootDecision {
    match (hash_usable, state) {
        (false, _) => BootDecision::Prompt,
        (true, None) => BootDecision::Adopt,
        (true, Some(s)) if s.acknowledged => BootDecision::Steady,
        (true, Some(_)) => BootDecision::RelockThenPrompt,
    }
}

/// Where the first-boot screen is (the wire `phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing to show.
    None,
    /// "Kiosk password not set, press M".
    Prompt,
    /// M was pressed, generation is in flight.
    Generating,
    /// The password is on screen.
    Showing,
    /// Acknowledged by the second M press.
    Done,
}

impl Phase {
    /// Phases in which the overlay owns the M button and the knob push.
    pub fn is_live(self) -> bool {
        matches!(self, Phase::Prompt | Phase::Generating | Phase::Showing)
    }
}

/// The phase machine and the one in-memory copy of the plaintext. No timers:
/// the only clock use is the ack debounce, measured from delivery.
pub struct FirstBoot {
    phase: Phase,
    secret: Option<Secret>,
    delivered_at: Option<Instant>,
}

impl FirstBoot {
    pub fn new(phase: Phase) -> FirstBoot {
        FirstBoot { phase, secret: None, delivered_at: None }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// First M press in `Prompt`. False (and no change) in any other phase.
    pub fn begin_generation(&mut self) -> bool {
        if self.phase == Phase::Prompt {
            self.phase = Phase::Generating;
            true
        } else {
            false
        }
    }

    /// Generation failed: back to the prompt, state stays unacknowledged.
    pub fn generation_failed(&mut self) {
        if self.phase == Phase::Generating {
            self.phase = Phase::Prompt;
        }
    }

    /// Generation succeeded: the password is held until the ack.
    pub fn generated(&mut self, secret: Secret) {
        if self.phase == Phase::Generating {
            self.phase = Phase::Showing;
            self.secret = Some(secret);
            self.delivered_at = None;
        }
    }

    /// The held password, only while `Showing`.
    pub fn secret(&self) -> Option<&Secret> {
        if self.phase == Phase::Showing {
            self.secret.as_ref()
        } else {
            None
        }
    }

    /// The secret reached a ready frontend at `now`. The earliest delivery
    /// since the last `clear_delivery` counts.
    pub fn mark_delivered(&mut self, now: Instant) {
        if self.phase == Phase::Showing && self.delivered_at.is_none() {
            self.delivered_at = Some(now);
        }
    }

    /// No ready frontend holds the secret any more.
    pub fn clear_delivery(&mut self) {
        self.delivered_at = None;
    }

    /// The second press may acknowledge: showing, delivered, and at least
    /// `min_gap` since delivery (an input debounce, not a display timeout).
    pub fn can_acknowledge(&self, now: Instant, min_gap: Duration) -> bool {
        self.phase == Phase::Showing && self.delivered_at.is_some_and(|at| now.saturating_duration_since(at) >= min_gap)
    }

    /// The acknowledgement was persisted: scrub memory, finish.
    pub fn acknowledged(&mut self) {
        if self.phase == Phase::Showing {
            self.phase = Phase::Done;
            self.secret = None;
            self.delivered_at = None;
        }
    }
}

impl fmt::Debug for FirstBoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirstBoot")
            .field("phase", &self.phase)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("delivered", &self.delivered_at.is_some())
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

    fn st(ack: bool) -> AccountState {
        AccountState { epoch: "e".repeat(32), kiosk_password_set: true, acknowledged: ack }
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
    fn hash_usability() {
        for unusable in ["", "!", "!!", "*", "!*"] {
            assert!(!hash_is_usable(unusable), "{unusable:?}");
        }
        for usable in ["$6$salt$hash", "!$6$salt$hash"] {
            assert!(hash_is_usable(usable), "{usable:?}");
        }
    }

    #[test]
    fn the_kiosk_hash_is_read_from_its_own_row() {
        let shadow = "root:*:1::::::\nkioskx:$6$a:1::::::\nkiosk:!:1::::::\n";
        assert_eq!(shadow_hash(shadow, "kiosk"), Some("!"));
        assert_eq!(shadow_hash(shadow, "kioskx"), Some("$6$a"));
        assert_eq!(shadow_hash(shadow, "nobody"), None);
    }

    #[test]
    fn boot_decision_covers_the_whole_table() {
        use BootDecision::*;
        assert_eq!(decide_boot(true, None), Adopt);
        assert_eq!(decide_boot(true, Some(&st(true))), Steady);
        assert_eq!(decide_boot(true, Some(&st(false))), RelockThenPrompt);
        for state in [None, Some(st(false)), Some(st(true))] {
            assert_eq!(decide_boot(false, state.as_ref()), Prompt);
        }
    }

    fn secret() -> Secret {
        Secret::new("Abcdefgh12345678")
    }

    #[test]
    fn the_phase_machine_walks_prompt_generating_showing_done() {
        let mut f = FirstBoot::new(Phase::Prompt);
        assert!(f.begin_generation());
        assert_eq!(f.phase(), Phase::Generating);
        assert!(!f.begin_generation(), "a second press while generating is ignored");
        assert!(f.secret().is_none());
        f.generated(secret());
        assert_eq!(f.phase(), Phase::Showing);
        assert_eq!(f.secret().unwrap().expose(), "Abcdefgh12345678");
        f.acknowledged();
        assert_eq!(f.phase(), Phase::Done);
        assert!(f.secret().is_none(), "memory scrubbed");
        assert!(!f.begin_generation());
    }

    #[test]
    fn a_failed_generation_returns_to_the_prompt() {
        let mut f = FirstBoot::new(Phase::Prompt);
        f.begin_generation();
        f.generation_failed();
        assert_eq!(f.phase(), Phase::Prompt);
        assert!(f.begin_generation(), "the next press tries again");
    }

    #[test]
    fn transitions_out_of_order_do_nothing() {
        for phase in [Phase::None, Phase::Prompt, Phase::Showing, Phase::Done] {
            let mut f = FirstBoot::new(phase);
            f.generated(secret());
            assert_eq!(f.phase(), phase);
            assert!(f.secret().is_none());
        }
        let mut f = FirstBoot::new(Phase::Prompt);
        f.acknowledged();
        f.generation_failed();
        assert_eq!(f.phase(), Phase::Prompt);
    }

    #[test]
    fn the_ack_needs_delivery_and_the_debounce_gap() {
        let t0 = Instant::now();
        let gap = Duration::from_millis(1500);
        let mut f = FirstBoot::new(Phase::Prompt);
        f.begin_generation();
        f.generated(secret());
        assert!(!f.can_acknowledge(t0 + Duration::from_secs(60), gap), "never delivered");
        f.mark_delivered(t0);
        assert!(!f.can_acknowledge(t0 + Duration::from_millis(1499), gap));
        assert!(f.can_acknowledge(t0 + Duration::from_millis(1500), gap));
        // A later delivery does not restart the debounce.
        f.mark_delivered(t0 + Duration::from_secs(10));
        assert!(f.can_acknowledge(t0 + Duration::from_millis(1500), gap));
        f.clear_delivery();
        assert!(!f.can_acknowledge(t0 + Duration::from_secs(60), gap));
    }

    #[test]
    fn liveness_of_phases() {
        assert!(!Phase::None.is_live() && !Phase::Done.is_live());
        assert!(Phase::Prompt.is_live() && Phase::Generating.is_live() && Phase::Showing.is_live());
    }

    #[test]
    fn first_boot_debug_never_prints_the_password() {
        let mut f = FirstBoot::new(Phase::Prompt);
        f.begin_generation();
        f.generated(secret());
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
