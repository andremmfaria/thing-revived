//! Ports: the traits the application layer depends on. Adapters implement
//! them; `run` is the only place that picks concrete types.

use crate::app::error::AppError;
use crate::app::hub::ConnId;
use crate::domain::accounts::model::Snapshot;
use crate::domain::accounts::password::Secret;
use crate::domain::accounts::state::{AccountFile, AccountState};
use crate::domain::bluetooth::{AgentEvent, PairedDevice};
use crate::domain::debug_access::Surface;
use crate::domain::device_settings as ds;
use crate::utils::types::AgentEventTx;
use crate::domain::settings::Settings;
use crate::domain::usb_role::{UsbRole, UsbRoleStatus};
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
    /// `userdel` refused because the account is logged in or has processes.
    Busy,
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
            AccountsError::Busy => f.write_str("user_busy"),
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
    /// Sets the hash to exactly `!` (no usable password, unlike `set_locked`,
    /// which keeps the old hash behind the lock prefix). Used to re-lock an
    /// unacknowledged first-boot password.
    fn clear_password(&self, user: &str) -> Result<(), AccountsError>;
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
}

/// The first-boot overlay's claim on the physical M button and the knob push,
/// consulted by `ButtonPolicy` on every press edge. Implemented by the users
/// service; only the controller's own evdev reader can call it, so a websocket
/// client can neither generate nor acknowledge.
pub trait FirstBootGate: Send + Sync {
    /// True only while the overlay is up (`prompt`, `generating`, `showing`)
    /// AND a frontend has announced itself with `user_bootstrap_get` and is
    /// still connected. Must not block.
    fn active(&self) -> bool;
    /// One consumed M press (the down edge). Advances the phase machine; any
    /// slow work runs on a worker thread, never on the caller.
    fn on_m_down(&self);
    /// Every down edge of the knob push, consumed or not. Feeds the account
    /// menu's global unlock cap: past the cap a fresh physical press is
    /// needed, so a local process can stall others but never the person at
    /// the device. Must not block.
    fn on_knob_press(&self) {}
}

/// Account, group and membership changes through the system tools
/// (`useradd`, `userdel`, `groupadd`, `groupdel`, `gpasswd`). Names are
/// validated by the adapter again; none of these ever sees a password.
pub trait AccountAdmin: Send + Sync {
    /// Creates the account locked (no password), with its private group and
    /// a home under the configured base.
    fn add_user(&self, name: &str) -> Result<(), AccountsError>;
    /// `userdel`, removing the home unless `keep_home`. A logged-in account
    /// is `AccountsError::Busy`.
    fn remove_user(&self, name: &str, keep_home: bool) -> Result<(), AccountsError>;
    fn add_group(&self, name: &str) -> Result<(), AccountsError>;
    fn remove_group(&self, name: &str) -> Result<(), AccountsError>;
    fn add_member(&self, user: &str, group: &str) -> Result<(), AccountsError>;
    fn remove_member(&self, user: &str, group: &str) -> Result<(), AccountsError>;
    /// Enables or disables the kiosk terminal (flag file plus
    /// `systemctl stop|start thing-revived-terminal-agent.service`), the
    /// extra step when the `kiosk` account is locked or unlocked.
    fn set_kiosk_terminal(&self, enabled: bool) -> Result<(), AccountsError>;
}

/// Reads the account files into a `Snapshot` (hashes reduced at the
/// boundary) and answers the questions the guard rails need from the disk.
pub trait AccountDb: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, AppError>;
    /// Days since the epoch, the unit `/etc/shadow` expiry uses.
    fn today_days(&self) -> i64;
    /// `<home_base>/<name>` or a key file for `name` already exists
    /// (`leftover_state`).
    fn leftover_state(&self, name: &str) -> Result<bool, AppError>;
}

/// Checks a password against the account's real hash. `Ok(false)` covers a
/// wrong password and every account that cannot authenticate (no hash,
/// locked); `Err` means the check itself could not run.
pub trait PasswordVerifier: Send + Sync {
    fn verify(&self, user: &str, password: &Secret) -> Result<bool, AppError>;
}

/// The menu-managed `authorized_keys` files, one per user, as lines.
pub trait AuthorizedKeysStore: Send + Sync {
    /// Key lines in file order; a missing file is empty.
    fn read(&self, user: &str) -> Result<Vec<String>, AppError>;
    /// Replaces the file atomically (mode 0600, owned by `owner`); no lines
    /// removes the file.
    fn write(&self, user: &str, lines: &[String], owner: (u32, u32)) -> Result<(), AppError>;
}

/// Turns one public key line into its `SHA256:...` fingerprint. An `Invalid`
/// error means the tool rejected the key; any other error means it could not
/// run.
pub trait KeyInspector: Send + Sync {
    fn fingerprint(&self, line: &str) -> Result<String, AppError>;
}

/// The USB-C port role (`superbird-usb-role`) and the reboot that applies a
/// persisted choice. Deliberately has no live switch: only the boot role is
/// ever changed.
pub trait UsbRoleCtl: Send + Sync {
    /// The running role and the persisted boot role.
    fn status(&self) -> Result<UsbRoleStatus, AppError>;
    /// Writes the boot role; applies from the next boot.
    fn persist(&self, role: UsbRole) -> Result<(), AppError>;
    fn reboot(&self) -> Result<(), AppError>;
    /// True on ADB dev builds, where the image forces device mode at boot
    /// and the persisted role is ignored.
    fn adb_forced(&self) -> bool;
}

/// One debug surface's systemd units (plan section 5.1). State is read
/// fresh with `systemctl is-active` on every call, never cached; `enable`
/// and `disable` each issue exactly one `systemctl start` or `stop` with
/// fixed argv, never from client input.
pub trait DebugAccessCtl: Send + Sync {
    /// Whether the surface is active right now.
    fn status(&self, surface: Surface) -> Result<bool, AppError>;
    /// Starts the surface's enabling unit.
    fn enable(&self, surface: Surface) -> Result<(), AppError>;
    /// Starts the surface's disabling unit (and, for UART, the stock getty
    /// it must hand the tty back to; see the adapter's own doc comment).
    fn disable(&self, surface: Surface) -> Result<(), AppError>;
    /// Whether `surface` starts enabled at boot (the image build flag),
    /// read fresh wherever the adapter can (plan 5.1/5.4; for adb this is
    /// covered separately by `UsbRoleCtl::adb_forced`, so this method only
    /// needs a real answer for uart). A default-provided method, so the
    /// existing `DebugAccessCli` keeps compiling unchanged until an
    /// adapter task wires a real `systemctl is-enabled` check; until then
    /// it fails safe by reporting "not enabled at boot", never a guess
    /// that could suppress a needed auto-disable.
    fn build_default(&self, _surface: Surface) -> Result<bool, AppError> {
        Ok(false)
    }
}

/// A narrow view onto the Users admin unlock session (plan section 5.1):
/// the one thing a debug-access mutation needs from `AccountsService`,
/// without a second unlock mechanism. Implemented by `AccountsService`
/// itself, delegating to its existing `Sessions::require`.
pub trait AdminSessionGate: Send + Sync {
    /// The admin name for a mutating command on this connection; refreshes
    /// the idle timer exactly like every other mutating accounts command.
    /// `Err` means no unlocked session on `conn` (expired or never
    /// unlocked); the caller reports this as `not_unlocked`.
    fn require(&self, conn: ConnId) -> Result<String, AppError>;
    /// Registers a callback invoked once for every connection whose admin
    /// session ends, for any reason (idle expiry, an explicit lock, or the
    /// connection closing). Used by debug-access to auto-disable a surface
    /// it enabled at runtime once the session that enabled it is gone
    /// (plan section 7's lingering-session recommendation, owner-accepted
    /// in section 12). The callback must not block and must not call back
    /// into the gate that is invoking it.
    fn on_session_end(&self, cb: Box<dyn Fn(ConnId) + Send + Sync>);
}

/// The persisted device settings file (plan section 4). The adapter does the
/// file I/O the shared loader (`domain::device_settings::load`) asks for:
/// read, seed, reseed, quarantine a corrupt file, atomic write.
pub trait DeviceSettingsStore: Send + Sync {
    /// Runs the shared loader against the real files and performs the write
    /// and quarantine it decided on. Never fails: every problem becomes
    /// fail-safe values plus notes (`Loaded::persistable` is false when the
    /// store itself is unusable).
    fn load(&self) -> ds::Loaded;
    /// One atomic write (temp file, fsync, rename, fsync directory, 0600)
    /// of all four values, so host plus adb off persist together.
    fn save(&self, image_id: &str, settings: &ds::DeviceSettings) -> Result<(), AppError>;
}

/// Live apply and live read for the device settings. Every method runs one
/// tool with fixed argv; no client text ever reaches an argument.
pub trait DeviceCtl: Send + Sync {
    /// The running USB role (`superbird-usb-role status`).
    fn usb_status(&self) -> Result<ds::UsbRoleStatus, AppError>;
    /// Whether the ssh socket, adbd or the uart root console is active
    /// right now (`systemctl is-active`, only exact `active` is on). `Mode`
    /// is not a unit: use `usb_status`.
    fn is_on(&self, setting: ds::Setting) -> Result<bool, AppError>;
    /// Starts or stops one of ssh, adb, uart. ssh on removes the `ssh-off`
    /// flag before starting the socket; ssh off creates it, then stops the
    /// socket and the `sshd@*` instances; adb off starts usb-bind; uart off
    /// stops the root console then starts the stock getty.
    fn set_live(&self, setting: ds::Setting, on: bool) -> Result<(), AppError>;
    /// `superbird-usb-role to-host --no-revert`.
    fn to_host(&self) -> Result<(), AppError>;
    /// `superbird-usb-role to-device`, plus re-binding the gadget NCM-only.
    fn to_device(&self) -> Result<(), AppError>;
    /// Mirrors the mode to the legacy `boot-role` file (`persist`).
    fn persist_boot_role(&self, role: ds::UsbRole) -> Result<(), AppError>;
    fn reboot(&self) -> Result<(), AppError>;
}
