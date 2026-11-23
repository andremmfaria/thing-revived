//! Error codes shared by the account menu and the first-boot flow, and the
//! mapping from each domain error to its wire code. The strings are the wire
//! contract (plan section 8, delta section 5.6); the protocol layer turns
//! them into JSON.

use super::guard::GuardError;
use super::keys::KeyError;
use super::names::NameError;
use super::password::PasswordError;
use super::ratelimit::Denied;
use super::session::SessionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotUnlocked,
    BadPassword,
    RateLimited,
    InvalidName,
    InvalidPassword,
    InvalidKey,
    Exists,
    LeftoverState,
    NotFound,
    SystemAccount,
    ProtectedAccount,
    LastAdmin,
    GroupNotAssignable,
    TooMany,
    ConfirmRequired,
    NoPassword,
    SessionsRemain,
    UserBusy,
    StorageUnavailable,
    Internal,
    /// First-boot flow: the generator or `chpasswd` failed.
    GenerateFailed,
    /// First-boot flow: the acknowledgement could not be written.
    AckFailed,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 22] = [
        ErrorCode::NotUnlocked,
        ErrorCode::BadPassword,
        ErrorCode::RateLimited,
        ErrorCode::InvalidName,
        ErrorCode::InvalidPassword,
        ErrorCode::InvalidKey,
        ErrorCode::Exists,
        ErrorCode::LeftoverState,
        ErrorCode::NotFound,
        ErrorCode::SystemAccount,
        ErrorCode::ProtectedAccount,
        ErrorCode::LastAdmin,
        ErrorCode::GroupNotAssignable,
        ErrorCode::TooMany,
        ErrorCode::ConfirmRequired,
        ErrorCode::NoPassword,
        ErrorCode::SessionsRemain,
        ErrorCode::UserBusy,
        ErrorCode::StorageUnavailable,
        ErrorCode::Internal,
        ErrorCode::GenerateFailed,
        ErrorCode::AckFailed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotUnlocked => "not_unlocked",
            ErrorCode::BadPassword => "bad_password",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::InvalidName => "invalid_name",
            ErrorCode::InvalidPassword => "invalid_password",
            ErrorCode::InvalidKey => "invalid_key",
            ErrorCode::Exists => "exists",
            ErrorCode::LeftoverState => "leftover_state",
            ErrorCode::NotFound => "not_found",
            ErrorCode::SystemAccount => "system_account",
            ErrorCode::ProtectedAccount => "protected_account",
            ErrorCode::LastAdmin => "last_admin",
            ErrorCode::GroupNotAssignable => "group_not_assignable",
            ErrorCode::TooMany => "too_many",
            ErrorCode::ConfirmRequired => "confirm_required",
            ErrorCode::NoPassword => "no_password",
            ErrorCode::SessionsRemain => "sessions_remain",
            ErrorCode::UserBusy => "user_busy",
            ErrorCode::StorageUnavailable => "storage_unavailable",
            ErrorCode::Internal => "internal",
            ErrorCode::GenerateFailed => "generate_failed",
            ErrorCode::AckFailed => "ack_failed",
        }
    }
}

impl From<NameError> for ErrorCode {
    fn from(e: NameError) -> ErrorCode {
        match e {
            NameError::Invalid | NameError::Reserved => ErrorCode::InvalidName,
            NameError::Exists => ErrorCode::Exists,
        }
    }
}

impl From<PasswordError> for ErrorCode {
    fn from(_: PasswordError) -> ErrorCode {
        ErrorCode::InvalidPassword
    }
}

impl From<KeyError> for ErrorCode {
    fn from(_: KeyError) -> ErrorCode {
        ErrorCode::InvalidKey
    }
}

impl From<SessionError> for ErrorCode {
    fn from(_: SessionError) -> ErrorCode {
        ErrorCode::NotUnlocked
    }
}

impl From<Denied> for ErrorCode {
    fn from(_: Denied) -> ErrorCode {
        ErrorCode::RateLimited
    }
}

impl From<GuardError> for ErrorCode {
    fn from(e: GuardError) -> ErrorCode {
        match e {
            GuardError::Name(n) => n.into(),
            GuardError::Password(p) => p.into(),
            GuardError::Key(k) => k.into(),
            GuardError::LeftoverState => ErrorCode::LeftoverState,
            GuardError::NotFound => ErrorCode::NotFound,
            GuardError::SystemAccount => ErrorCode::SystemAccount,
            GuardError::ProtectedAccount => ErrorCode::ProtectedAccount,
            GuardError::LastAdmin => ErrorCode::LastAdmin,
            GuardError::GroupNotAssignable => ErrorCode::GroupNotAssignable,
            GuardError::TooMany => ErrorCode::TooMany,
            GuardError::ConfirmRequired => ErrorCode::ConfirmRequired,
            GuardError::NoPassword => ErrorCode::NoPassword,
            GuardError::NotAdmin => ErrorCode::BadPassword,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn wire_strings_are_unique_snake_case_and_exclude_removed_codes() {
        let strs: BTreeSet<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(strs.len(), ErrorCode::ALL.len());
        for s in &strs {
            assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{s}");
        }
        for gone in ["bad_ack", "press_required", "expired", "not_pending"] {
            assert!(!strs.contains(gone), "{gone} was removed by the delta");
        }
    }

    #[test]
    fn domain_errors_map_to_plan_codes() {
        assert_eq!(ErrorCode::from(NameError::Reserved).as_str(), "invalid_name");
        assert_eq!(ErrorCode::from(NameError::Exists).as_str(), "exists");
        assert_eq!(ErrorCode::from(PasswordError::TooShort).as_str(), "invalid_password");
        assert_eq!(ErrorCode::from(GuardError::LastAdmin).as_str(), "last_admin");
        assert_eq!(ErrorCode::from(GuardError::Name(NameError::Invalid)).as_str(), "invalid_name");
        assert_eq!(ErrorCode::from(SessionError::NotUnlocked).as_str(), "not_unlocked");
        assert_eq!(ErrorCode::from(Denied::NeedPress).as_str(), "rate_limited");
        assert_eq!(ErrorCode::from(KeyError::Empty).as_str(), "invalid_key");
    }
}
