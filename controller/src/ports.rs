//! Ports: the traits the application layer depends on. Adapters implement
//! them; `run` is the only place that picks concrete types.

use crate::app::error::AppError;
use crate::domain::accounts::password::Secret;
use crate::domain::accounts::state::{AccountFile, AccountState};
use crate::domain::bluetooth::{AgentEvent, PairedDevice};
use crate::utils::types::AgentEventTx;
use crate::domain::settings::Settings;
use crate::protocol::BridgeEvent;
use std::sync::mpsc::Sender;

/// Where services send events for clients. Implemented by `app::hub::EventHub`
/// and, in tests, by `app::hub::RecordingSink`.
pub trait EventSink: Send + Sync {
    fn emit(&self, e: BridgeEvent);
}

/// Screen brightness in the raw sysfs range (0..=max).
pub trait Backlight: Send + Sync {
    fn max(&self) -> Result<u32, AppError>;
    fn get(&self) -> Result<u32, AppError>;
    /// Validates against the live maximum before writing.
    fn set(&self, value: u32) -> Result<(), AppError>;
}

/// Persisted menu settings. `load` never fails: any problem yields defaults.
pub trait SettingsStore: Send + Sync {
    fn load(&self) -> Settings;
    fn save(&self, settings: &Settings) -> Result<(), AppError>;
}

/// System timezone control. `set` does not validate the name; the service
/// checks it against `list` first.
pub trait TimezoneCtl: Send + Sync {
    fn list(&self) -> Vec<String>;
    fn get(&self) -> Result<String, AppError>;
    fn set(&self, tz: &str) -> Result<(), AppError>;
}

/// Restarts a system service unit (the KEY_M long-press recovery).
pub trait Restarter: Send + Sync {
    fn restart(&self, unit: &str) -> Result<(), AppError>;
}

/// BlueZ control: adapter power, discovery, pairing and the paired set.
/// Asynchronous facts (passkeys, scan results, pairing outcome) come back
/// through the `AgentEvent` channel, never as return values.
pub trait BluetoothPort: Send + Sync {
    fn get_powered(&self) -> Result<bool, AppError>;
    fn set_powered(&self, on: bool) -> Result<(), AppError>;
    fn start_scan(&self, events: AgentEventTx) -> Result<(), AppError>;
    fn stop_scan(&self) -> Result<(), AppError>;
    /// Blocks until BlueZ resolves the pairing; the outcome is reported as
    /// `AgentEvent::PairResult` on `events`. `Err` means it could not even start.
    fn pair(&self, address: &str, events: &Sender<AgentEvent>) -> Result<(), AppError>;
    /// Delivers the client's yes/no to the agent's blocked confirmation.
    fn submit_confirm(&self, accept: bool) -> Result<(), AppError>;
    fn list_paired(&self) -> Result<Vec<PairedDevice>, AppError>;
    fn connect(&self, address: &str) -> Result<(), AppError>;
    fn disconnect(&self, address: &str) -> Result<(), AppError>;
    fn forget(&self, address: &str) -> Result<(), AppError>;
}

/// Re-injects a key event into the system input stack (the uinput virtual
/// keyboard) so a grabbed key keeps acting like an ordinary keystroke.
pub trait KeyReplay: Send {
    fn replay(&mut self, code: u16, value: i32) -> Result<(), AppError>;
}

/// Time source for the long-press timer, injectable so tests need no real waits.
pub trait Clock: Send + Sync {
    fn now(&self) -> std::time::Instant;
    /// Blocks the calling thread until `deadline` (returns at once if past).
    fn sleep_until(&self, deadline: std::time::Instant);
}

/// Source of random bytes. `fill` either fills the whole buffer or fails.
pub trait Entropy: Send + Sync {
    fn fill(&self, buf: &mut [u8]) -> Result<(), AppError>;
}

/// Monotonic seconds since boot, the clock the first-boot expiry uses (the
/// tmpfs holder does not survive a reboot, so wall time is never needed).
pub trait MonotonicClock: Send + Sync {
    fn now_secs(&self) -> u64;
}

/// A password read back from the tmpfs holder.
pub struct StoredSecret {
    pub secret: Secret,
    /// Absolute deadline in `MonotonicClock` seconds.
    pub expires_at_secs: u64,
}

/// The tmpfs holder of the one-time plaintext password.
pub trait SecretStore: Send + Sync {
    /// Replaces any previous secret.
    fn store(&self, secret: &Secret, expires_at_secs: u64) -> Result<(), AppError>;
    /// `Ok(None)` when absent or unreadable as a secret.
    fn load(&self) -> Result<Option<StoredSecret>, AppError>;
    /// Overwrites and removes the holder. Absent is not an error.
    fn scrub(&self) -> Result<(), AppError>;
}

/// `state.json` on the data partition.
pub trait AccountStateStore: Send + Sync {
    /// `Ok(None)` when missing or corrupt.
    fn load(&self) -> Result<Option<AccountState>, AppError>;
    fn save(&self, state: &AccountState) -> Result<(), AppError>;
}

/// Why an account operation was refused. `NoPassword` and `SessionsRemain`
/// map to wire codes of the same name; anything else is `Tool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountsError {
    InvalidName,
    InvalidPassword,
    /// The account has no usable hash, so it cannot be unlocked.
    NoPassword,
    /// Processes of the account survived the kill sequence.
    SessionsRemain,
    NotFound,
    Tool(AppError),
}

impl std::fmt::Display for AccountsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountsError::InvalidName => f.write_str("invalid account name"),
            AccountsError::InvalidPassword => f.write_str("invalid password"),
            AccountsError::NoPassword => f.write_str("no_password"),
            AccountsError::SessionsRemain => f.write_str("sessions_remain"),
            AccountsError::NotFound => f.write_str("not_found"),
            AccountsError::Tool(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AccountsError {}

impl From<AccountsError> for AppError {
    fn from(e: AccountsError) -> AppError {
        match e {
            AccountsError::Tool(inner) => inner,
            AccountsError::InvalidName | AccountsError::InvalidPassword => AppError::invalid(e.to_string()),
            other => AppError::failed(other),
        }
    }
}

/// Account changes through the system tools. Passwords travel on stdin only.
pub trait AccountsPort: Send + Sync {
    /// Sets the password (SHA-512 crypt). The account must exist.
    fn set_password(&self, user: &str, password: &Secret) -> Result<(), AccountsError>;
    /// Locks (`usermod --lock --expiredate 1`) or unlocks. Unlocking an
    /// account with no usable hash is refused with `NoPassword`.
    fn set_locked(&self, user: &str, locked: bool) -> Result<(), AccountsError>;
    /// Ends every session and process of `user`; `SessionsRemain` if any
    /// survive.
    fn end_sessions(&self, user: &str) -> Result<(), AccountsError>;
}

/// The files `provision` reconciles. `storage_available` is false while the
/// `/etc` overlay is not up (degraded mode); nothing else may be called then.
pub trait AccountFiles: Send + Sync {
    fn storage_available(&self) -> bool;
    /// A file that does not exist reads as empty.
    fn read_lower(&self, file: AccountFile) -> Result<String, AppError>;
    fn read_current(&self, file: AccountFile) -> Result<String, AppError>;
    /// Appends whole lines (a missing final newline is added first) with an
    /// atomic replace that keeps mode and owner.
    fn append_lines(&self, file: AccountFile, lines: &[String]) -> Result<(), AppError>;
    /// Names of the entries in the overlay's upper directory.
    fn list_upper(&self) -> Result<Vec<String>, AppError>;
    /// Names of the entries in the menu-managed key store.
    fn list_key_files(&self) -> Result<Vec<String>, AppError>;
    fn delete_key_file(&self, name: &str) -> Result<(), AppError>;
    /// Whether the image's baked authorized key is in place.
    fn baked_key_present(&self) -> bool;
}
