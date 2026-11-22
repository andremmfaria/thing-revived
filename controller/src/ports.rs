//! Ports: the traits the application layer depends on. Adapters implement
//! them; `run` is the only place that picks concrete types.

use crate::app::error::AppError;
use crate::domain::bluetooth::{AgentEvent, PairedDevice};
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
    fn start_scan(&self, events: Sender<AgentEvent>) -> Result<(), AppError>;
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
