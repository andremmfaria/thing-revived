//! Account and group name rules. Pure; existence checks take the names the
//! caller already read from its snapshot.

use std::fmt;

/// The reserved account: cannot be removed or renamed.
pub const KIOSK_USER: &str = "kiosk";

/// Names nobody may create, on top of whatever already exists. Anything
/// starting with `systemd-` is reserved too (see `is_reserved`).
pub const RESERVED: &[&str] = &[
    "root", "nobody", KIOSK_USER, "sudo", "adm", "wheel", "shadow", "disk", "kmem", "tty", "utmp", "staff", "sshd",
    "messagebus",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Fails `^[a-z_][a-z0-9_-]{0,30}$`.
    Invalid,
    Reserved,
    /// A user or group with this name exists already.
    Exists,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NameError::Invalid => "name must be 1 to 31 characters of a-z, 0-9, _ and -, not starting with a digit or -",
            NameError::Reserved => "this name is reserved",
            NameError::Exists => "a user or group with this name already exists",
        })
    }
}

impl std::error::Error for NameError {}

/// `^[a-z_][a-z0-9_-]{0,30}$`, checked by hand (no regex dependency).
pub fn is_valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(b'a'..=b'z') | Some(b'_') => {}
        _ => return false,
    }
    name.len() <= 31 && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name) || name.starts_with("systemd-")
}

/// Full check for a name about to be created (user or group).
/// `taken` is every existing user name and group name.
pub fn validate_new<'a>(name: &str, taken: impl IntoIterator<Item = &'a str>) -> Result<(), NameError> {
    if !is_valid_name(name) {
        return Err(NameError::Invalid);
    }
    if is_reserved(name) {
        return Err(NameError::Reserved);
    }
    if taken.into_iter().any(|t| t == name) {
        return Err(NameError::Exists);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_regex_matches() {
        for n in ["a", "_x", "alice", "a-b_c9", &"a".repeat(31)] {
            assert!(is_valid_name(n), "{n}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for n in ["", "A", "1a", "-a", "a b", "a.b", "a:b", "a\n", "é", &"a".repeat(32)] {
            assert!(!is_valid_name(n), "{n:?}");
        }
    }

    #[test]
    fn reserved_names_and_prefix() {
        assert_eq!(validate_new("root", []), Err(NameError::Reserved));
        assert_eq!(validate_new("kiosk", []), Err(NameError::Reserved));
        assert_eq!(validate_new("systemd-foo", []), Err(NameError::Reserved));
        assert_eq!(validate_new("systemdx", []), Ok(()));
    }

    #[test]
    fn existing_and_order_of_checks() {
        assert_eq!(validate_new("bob", ["alice", "bob"]), Err(NameError::Exists));
        assert_eq!(validate_new("Bob", ["Bob"]), Err(NameError::Invalid));
        assert_eq!(validate_new("carol", ["alice"]), Ok(()));
    }
}
