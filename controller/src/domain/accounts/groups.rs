//! Group policy (Phase 2b): which groups the menu may assign, which need
//! `confirm`, and the rules for creating, removing and changing membership.
//! Pure over a `Snapshot`.

use super::guard::{check_mutation, mutable_target, GuardError, Limits, Mutation};
use super::model::{is_managed_id, is_system_id, Snapshot, ADMIN_GROUP};
use super::names;

/// Never assignable, whatever the config says (plan section 6).
pub const NEVER_ASSIGNABLE: &[&str] =
    &["root", "shadow", "disk", "kmem", "tty", "utmp", "staff", "sshd", "messagebus"];

/// `[users] assignable_system_groups` default.
pub const DEFAULT_ASSIGNABLE_SYSTEM: &[&str] = &["sudo", "adm"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPolicy {
    assignable_system: Vec<String>,
}

impl Default for GroupPolicy {
    fn default() -> Self {
        GroupPolicy::new(DEFAULT_ASSIGNABLE_SYSTEM.iter().map(|s| s.to_string()).collect())
    }
}

fn never_assignable(name: &str) -> bool {
    NEVER_ASSIGNABLE.contains(&name) || name.starts_with("systemd-")
}

impl GroupPolicy {
    pub fn new(assignable_system: Vec<String>) -> GroupPolicy {
        GroupPolicy { assignable_system }
    }

    /// Managed groups (gid 1000 to 59999) always; system groups only when
    /// configured and not on the never list. Users' private groups are
    /// excluded: membership of a user's own group is not a menu operation.
    pub fn is_assignable(&self, snap: &Snapshot, group: &str) -> bool {
        if never_assignable(group) {
            return false;
        }
        match snap.group(group) {
            None => false,
            Some(g) if is_managed_id(g.gid) => snap.user(group).is_none(),
            Some(g) if is_system_id(g.gid) => self.assignable_system.iter().any(|s| s == group),
            Some(_) => false,
        }
    }

    /// `sudo` grants admin, and every configured system group beyond the
    /// default `adm` is a config-time exposure: both need `confirm`.
    pub fn needs_confirm(&self, group: &str) -> bool {
        group == ADMIN_GROUP || (group != "adm" && self.assignable_system.iter().any(|s| s == group))
    }
}

/// Add `user` to `group`. Returns whether `confirm` is required, so the
/// caller can run `require_confirm`.
pub fn check_add_member(
    snap: &Snapshot,
    policy: &GroupPolicy,
    user: &str,
    group: &str,
) -> Result<bool, GuardError> {
    mutable_target(snap, user)?;
    if !policy.is_assignable(snap, group) {
        return Err(GuardError::GroupNotAssignable);
    }
    Ok(policy.needs_confirm(group))
}

/// Remove `user` from `group`. Leaving `sudo` is a demotion and cannot take
/// the last effective admin.
pub fn check_remove_member(
    snap: &Snapshot,
    policy: &GroupPolicy,
    today_days: i64,
    user: &str,
    group: &str,
) -> Result<(), GuardError> {
    mutable_target(snap, user)?;
    if !policy.is_assignable(snap, group) {
        return Err(GuardError::GroupNotAssignable);
    }
    if !snap.group(group).is_some_and(|g| g.members.iter().any(|m| m == user)) {
        return Err(GuardError::NotFound);
    }
    if group == ADMIN_GROUP {
        check_mutation(snap, today_days, user, Mutation::Demote)?;
    }
    Ok(())
}

pub fn check_create_group(snap: &Snapshot, name: &str, limits: &Limits) -> Result<(), GuardError> {
    names::validate_new(name, snap.taken_names())?;
    if snap.managed_group_count() >= limits.max_groups {
        return Err(GuardError::TooMany);
    }
    Ok(())
}

/// Only owner-created groups can be removed; `confirm` is always required
/// (the bool is returned for symmetry with `check_add_member`).
pub fn check_remove_group(snap: &Snapshot, name: &str) -> Result<bool, GuardError> {
    let g = snap.group(name).ok_or(GuardError::NotFound)?;
    if is_system_id(g.gid) {
        return Err(GuardError::SystemAccount);
    }
    // A user's private group goes with the user, never on its own.
    if snap.user(name).is_some() || !is_managed_id(g.gid) {
        return Err(GuardError::GroupNotAssignable);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::super::model::fixtures::{snap, TODAY};
    use super::*;

    #[test]
    fn assignability() {
        let s = snap();
        let p = GroupPolicy::default();
        assert!(p.is_assignable(&s, "sudo"));
        assert!(p.is_assignable(&s, "adm"));
        assert!(p.is_assignable(&s, "staff2"), "managed group");
        assert!(!p.is_assignable(&s, "alice"), "private group");
        assert!(!p.is_assignable(&s, "root"));
        assert!(!p.is_assignable(&s, "ghost"));
        // a configured name on the never list stays refused
        let greedy = GroupPolicy::new(vec!["shadow".into(), "root".into(), "adm".into()]);
        let mut s2 = s;
        s2.groups.push(super::super::model::Group { name: "shadow".into(), gid: 42, members: vec![] });
        assert!(!greedy.is_assignable(&s2, "shadow"));
        assert!(!greedy.is_assignable(&s2, "root"));
        assert!(!greedy.is_assignable(&s2, "sudo"), "not configured");
    }

    #[test]
    fn device_groups_need_config_and_confirm() {
        let mut s = snap();
        s.groups.push(super::super::model::Group { name: "input".into(), gid: 105, members: vec![] });
        let def = GroupPolicy::default();
        assert_eq!(check_add_member(&s, &def, "alice", "input"), Err(GuardError::GroupNotAssignable));
        let cfg = GroupPolicy::new(vec!["sudo".into(), "adm".into(), "input".into()]);
        assert_eq!(check_add_member(&s, &cfg, "alice", "input"), Ok(true));
        assert_eq!(check_add_member(&s, &cfg, "alice", "adm"), Ok(false));
        assert_eq!(check_add_member(&s, &cfg, "alice", "sudo"), Ok(true));
        assert_eq!(check_add_member(&s, &cfg, "alice", "staff2"), Ok(false));
    }

    #[test]
    fn membership_targets() {
        let s = snap();
        let p = GroupPolicy::default();
        assert_eq!(check_add_member(&s, &p, "root", "adm"), Err(GuardError::SystemAccount));
        assert_eq!(check_add_member(&s, &p, "ghost", "adm"), Err(GuardError::NotFound));
        assert_eq!(check_remove_member(&s, &p, TODAY, "alice", "adm"), Err(GuardError::NotFound), "not a member");
    }

    #[test]
    fn leaving_sudo_cannot_remove_the_last_admin() {
        let mut s = snap();
        let p = GroupPolicy::default();
        assert_eq!(check_remove_member(&s, &p, TODAY, "alice", "sudo"), Ok(()));
        // alice loses her password: kiosk is the only effective admin
        s.shadow.iter_mut().find(|x| x.name == "alice").unwrap().has_password = false;
        assert_eq!(check_remove_member(&s, &p, TODAY, "kiosk", "sudo"), Err(GuardError::LastAdmin));
        // removing a non-admin-affecting group is fine
        assert_eq!(check_remove_member(&s, &p, TODAY, "kiosk", "adm"), Ok(()));
    }

    #[test]
    fn create_group_rules() {
        let s = snap();
        let l = Limits::default();
        assert_eq!(check_create_group(&s, "team", &l), Ok(()));
        assert!(matches!(check_create_group(&s, "alice", &l), Err(GuardError::Name(_))));
        assert!(matches!(check_create_group(&s, "Bad Name", &l), Err(GuardError::Name(_))));
        assert!(matches!(check_create_group(&s, "sudo", &l), Err(GuardError::Name(_))));
        let tight = Limits { max_groups: 1, ..l };
        assert_eq!(check_create_group(&s, "team", &tight), Err(GuardError::TooMany));
    }

    #[test]
    fn remove_group_rules() {
        let s = snap();
        assert_eq!(check_remove_group(&s, "staff2"), Ok(true));
        assert_eq!(check_remove_group(&s, "sudo"), Err(GuardError::SystemAccount));
        assert_eq!(check_remove_group(&s, "alice"), Err(GuardError::GroupNotAssignable));
        assert_eq!(check_remove_group(&s, "nope"), Err(GuardError::NotFound));
    }
}
