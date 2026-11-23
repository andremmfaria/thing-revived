//! Guard rails for account mutations (plan section 6). Pure functions over a
//! `Snapshot`; each returns a typed `GuardError` that `events` maps to a wire
//! code. The caller checks the session first (`session`), then these.

use super::keys::KeyError;
use super::model::{is_system_id, Snapshot, User};
use super::names::{self, NameError, KIOSK_USER};
use super::password::{self, PasswordError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardError {
    Name(NameError),
    Password(PasswordError),
    Key(KeyError),
    /// `/var/home/<name>` or a key file already exists.
    LeftoverState,
    NotFound,
    /// Target uid below 1000.
    SystemAccount,
    /// Remove or rename `kiosk`.
    ProtectedAccount,
    /// Would leave no effective admin.
    LastAdmin,
    GroupNotAssignable,
    TooMany,
    ConfirmRequired,
    /// The account has no usable password hash.
    NoPassword,
    /// Unlock attempted for an account that is not an effective admin.
    /// Wire code is `bad_password` so the reply does not say which it was.
    NotAdmin,
}

impl From<NameError> for GuardError {
    fn from(e: NameError) -> Self {
        GuardError::Name(e)
    }
}

impl From<PasswordError> for GuardError {
    fn from(e: PasswordError) -> Self {
        GuardError::Password(e)
    }
}

impl From<KeyError> for GuardError {
    fn from(e: KeyError) -> Self {
        GuardError::Key(e)
    }
}

/// `[users]` count limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_users: usize,
    pub max_groups: usize,
    pub max_keys_per_user: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_users: 16, max_groups: 32, max_keys_per_user: 20 }
    }
}

/// What a mutation does to the target's admin standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    Remove,
    Lock,
    /// Leave group `sudo`.
    Demote,
    /// Clear or invalidate the password hash.
    BlankPassword,
}

/// Missing or mismatched `confirm` on a destructive command.
pub fn require_confirm(confirmed: bool) -> Result<(), GuardError> {
    if confirmed {
        Ok(())
    } else {
        Err(GuardError::ConfirmRequired)
    }
}

/// A target for any mutation: it must exist and must not be a system user.
pub fn mutable_target<'a>(snap: &'a Snapshot, name: &str) -> Result<&'a User, GuardError> {
    let user = snap.user(name).ok_or(GuardError::NotFound)?;
    if is_system_id(user.uid) {
        return Err(GuardError::SystemAccount);
    }
    Ok(user)
}

/// Remove, lock, demote or blank the password of `name`. `kiosk` can never
/// be removed; every other case is refused only when `name` is the last
/// effective admin.
pub fn check_mutation(snap: &Snapshot, today_days: i64, name: &str, what: Mutation) -> Result<(), GuardError> {
    mutable_target(snap, name)?;
    if what == Mutation::Remove && name == KIOSK_USER {
        return Err(GuardError::ProtectedAccount);
    }
    let admins = snap.effective_admins(today_days);
    if admins.contains(&name) && admins.len() == 1 {
        return Err(GuardError::LastAdmin);
    }
    Ok(())
}

/// `user_add`: name rules, count limit, leftover state, optional password.
/// `password` is `None` when the service generates one (generated passwords
/// pass the validator by construction).
pub fn check_add_user(
    snap: &Snapshot,
    name: &str,
    password: Option<&str>,
    leftover_home_or_key: bool,
    limits: &Limits,
) -> Result<(), GuardError> {
    names::validate_new(name, snap.taken_names())?;
    if leftover_home_or_key {
        return Err(GuardError::LeftoverState);
    }
    if snap.managed_user_count() >= limits.max_users {
        return Err(GuardError::TooMany);
    }
    if let Some(p) = password {
        password::validate(name, p)?;
    }
    Ok(())
}

/// `user_set_password` on `name`. Own-password `current_password` and the
/// `confirm` rule for another user's account are decided by the caller with
/// `require_confirm`.
pub fn check_set_password(snap: &Snapshot, name: &str, new_password: Option<&str>) -> Result<(), GuardError> {
    mutable_target(snap, name)?;
    if let Some(p) = new_password {
        password::validate(name, p)?;
    }
    Ok(())
}

/// Who may start an unlock session: an effective admin with a password.
pub fn check_can_unlock(snap: &Snapshot, today_days: i64, name: &str) -> Result<(), GuardError> {
    // A passwordless admin is reported like any other refusal (`bad_password`):
    // the answer must not reveal which admin has no password.
    if snap.user(name).is_none() || !snap.has_password(name) || !snap.is_effective_admin(name, today_days) {
        return Err(GuardError::NotAdmin);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::events::ErrorCode;
    use super::super::model::fixtures::{snap, TODAY};
    use super::*;

    fn lock(s: &mut Snapshot, name: &str) {
        s.shadow.iter_mut().find(|x| x.name == name).unwrap().locked = true;
    }

    #[test]
    fn system_accounts_are_never_modifiable() {
        let s = snap();
        for m in [Mutation::Remove, Mutation::Lock, Mutation::Demote, Mutation::BlankPassword] {
            assert_eq!(check_mutation(&s, TODAY, "root", m), Err(GuardError::SystemAccount));
        }
        assert_eq!(check_set_password(&s, "root", None), Err(GuardError::SystemAccount));
        assert_eq!(check_mutation(&s, TODAY, "ghost", Mutation::Lock), Err(GuardError::NotFound));
    }

    #[test]
    fn kiosk_cannot_be_removed_even_with_other_admins() {
        let s = snap();
        assert_eq!(check_mutation(&s, TODAY, "kiosk", Mutation::Remove), Err(GuardError::ProtectedAccount));
        // locking kiosk is fine while alice is an admin
        assert_eq!(check_mutation(&s, TODAY, "kiosk", Mutation::Lock), Ok(()));
    }

    #[test]
    fn last_admin_blocks_every_mutation() {
        let mut s = snap();
        lock(&mut s, "alice");
        for m in [Mutation::Lock, Mutation::Demote, Mutation::BlankPassword] {
            assert_eq!(check_mutation(&s, TODAY, "kiosk", m), Err(GuardError::LastAdmin), "{m:?}");
        }
        // a non-admin is unaffected, and removing the locked alice is fine
        assert_eq!(check_mutation(&s, TODAY, "bob", Mutation::Remove), Ok(()));
        assert_eq!(check_mutation(&s, TODAY, "alice", Mutation::Remove), Ok(()));
    }

    #[test]
    fn add_user_checks_in_order() {
        let s = snap();
        let l = Limits::default();
        let ok = "Correct-Horse-9";
        assert_eq!(check_add_user(&s, "carol", Some(ok), false, &l), Ok(()));
        assert_eq!(check_add_user(&s, "carol", None, false, &l), Ok(()));
        assert_eq!(check_add_user(&s, "Carol", Some(ok), false, &l), Err(GuardError::Name(NameError::Invalid)));
        assert_eq!(check_add_user(&s, "alice", Some(ok), false, &l), Err(GuardError::Name(NameError::Exists)));
        assert_eq!(check_add_user(&s, "staff2", Some(ok), false, &l), Err(GuardError::Name(NameError::Exists)));
        assert_eq!(check_add_user(&s, "carol", Some(ok), true, &l), Err(GuardError::LeftoverState));
        assert_eq!(
            check_add_user(&s, "carol", Some("short"), false, &l),
            Err(GuardError::Password(PasswordError::TooShort))
        );
        assert_eq!(
            check_add_user(&s, "carol", Some("aaaaaaaaaaaaaaaa"), false, &l),
            Err(GuardError::Password(PasswordError::SingleClass))
        );
        let tight = Limits { max_users: 3, ..l };
        assert_eq!(check_add_user(&s, "carol", Some(ok), false, &tight), Err(GuardError::TooMany));
    }

    #[test]
    fn unlock_needs_an_effective_admin_with_a_password() {
        let mut s = snap();
        assert_eq!(check_can_unlock(&s, TODAY, "alice"), Ok(()));
        assert_eq!(check_can_unlock(&s, TODAY, "bob"), Err(GuardError::NotAdmin));
        assert_eq!(check_can_unlock(&s, TODAY, "ghost"), Err(GuardError::NotAdmin));
        assert_eq!(check_can_unlock(&s, TODAY, "root"), Err(GuardError::NotAdmin), "no password looks like any other refusal");
        lock(&mut s, "alice");
        assert_eq!(check_can_unlock(&s, TODAY, "alice"), Err(GuardError::NotAdmin));
        assert_eq!(ErrorCode::from(GuardError::NotAdmin), ErrorCode::BadPassword);
    }

    #[test]
    fn confirm_gate() {
        assert_eq!(require_confirm(true), Ok(()));
        assert_eq!(require_confirm(false), Err(GuardError::ConfirmRequired));
    }
}
