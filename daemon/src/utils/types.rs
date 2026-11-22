//! Central catalogue of the crate's shared types.
//!
//! `utils::types::*` is the single index of every type the crate exposes.
//! Convention:
//!
//! * Enums, traits, type aliases and types used by two or more modules are
//!   DEFINED here.
//! * A struct private to one module and tightly coupled to its impl stays
//!   with the impl and is only re-exported here (`pub use`), as are the serde
//!   wire messages (`ClientMsg`, `ServerMsg`, `SessionInfo`), which stay in
//!   `protocol.rs` next to their encoder and decoder.
//! * Modules import shared types from `crate::utils::types`, not from each
//!   other's internals. The re-exports are name-level only; no module logic
//!   depends on another through this file.
//!
//! | Type | Kind | Defined in |
//! |------|------|------------|
//! | `SessionId`, `Msg`, `SharedClock` | aliases | here |
//! | `ControlAction`, `ClockStatus` | wire enums | here (serde attrs unchanged) |
//! | `Decision` | enum (respawn policy) | here |
//! | `ReadOutcome`, `GroupSignal` | enums (pty) | here |
//! | `ConfigError` | error enum | here |
//! | `StartupError` | error struct | here |
//! | `ClientMsg`, `ServerMsg`, `SessionInfo` | wire messages | `protocol` |
//! | `Config`, `SessionConfig`, `ServerConfig`, `LoggingConfig`, `PtyConfig`, `RespawnConfig`, `ClockConfig`, `Loaded` | config | `utils::config` |
//! | `PtyMaster`, `PtyReader` | pty handles | `pty` |
//! | `RespawnPolicy` | pure policy | `respawn_policy` |
//! | `ClientHub` | fanout | `hub` |
//! | `Session` | child lifecycle | `session` |
//! | `ServerCtx` | accept loop context | `server` |
//! | `ClockState` | clock sync state | `clock` |
//! | `Utf8Carry` | chunk-safe decoder | `utf8` |
//!
//! Constants (config-independent names): `BIND_ENV`,
//! `CONFIG_ENV`, `DEFAULT_CONFIG_PATH`, `DEFAULT_BIND`, `EXIT_CONFIG`, all
//! re-exported from their owning module. There are no traits in the crate
//! yet; new ones belong here.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::clock::ClockState;
pub use crate::hub::ClientHub;
pub use crate::net::BIND_ENV;
pub use crate::protocol::{ClientMsg, ServerMsg, SessionInfo};
pub use crate::pty::{PtyMaster, PtyReader};
pub use crate::respawn_policy::RespawnPolicy;
pub use crate::server::ServerCtx;
pub use crate::session::Session;
pub use crate::utf8::Utf8Carry;
pub use crate::utils::config::{
    ClockConfig, Config, Loaded, LoggingConfig, PtyConfig, RespawnConfig, ServerConfig,
    SessionConfig, CONFIG_ENV, DEFAULT_BIND, DEFAULT_CONFIG_PATH, EXIT_CONFIG,
};

/// Index of a session in the configured list; also its wire identity.
pub type SessionId = usize;

/// Shared message: one serialization, cheap per-client clone under the lock.
pub type Msg = Arc<str>;

/// Clock state shared between the sync thread and client threads.
pub type SharedClock = Arc<Mutex<ClockState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Interrupt,
    Clear,
    Respawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClockStatus {
    Unsynced,
    Ok,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Respawn after `delay`; `attempt` is the 1-based count within the window.
    Retry { attempt: usize, delay: Duration },
    /// Crash-looping: stop auto-respawning; `attempt` exits within the window.
    GiveUp { attempt: usize },
}

/// Result of one `PtyReader::read_timeout` call.
#[derive(Debug)]
pub enum ReadOutcome {
    /// `n > 0` bytes were read into the buffer.
    Data(usize),
    /// Nothing became readable within the timeout.
    Idle,
    /// Clean EOF, or EIO (the common Linux "slave closed" signal): the child
    /// side is gone.
    Eof,
    /// Interrupted by a signal; just call again.
    Interrupted,
    Failed(std::io::Error),
}

/// Signals sent to a child's whole process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSignal {
    Interrupt,
    Kill,
}

#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, message: String },
    Invalid { path: PathBuf, message: String },
}

/// A startup failure: what to print and which exit code to use.
#[derive(Debug)]
pub struct StartupError {
    pub exit_code: i32,
    pub message: String,
}

#[cfg(test)]
mod tests {
    //! Touches every catalogued name so removing one breaks compilation.
    use super::*;

    #[test]
    fn catalogue_names_resolve() {
        fn _types(
            _: (Option<ClientMsg>, Option<ServerMsg>, Option<SessionInfo>, Option<Config>),
            _: (Option<SessionConfig>, Option<ServerConfig>, Option<LoggingConfig>, Option<PtyConfig>),
            _: (Option<RespawnConfig>, Option<ClockConfig>, Option<Loaded>, Option<PtyMaster>),
            _: (Option<PtyReader>, Option<RespawnPolicy>, Option<ClientHub>, Option<Session>),
            _: (Option<ServerCtx>, Option<ClockState>, Option<Utf8Carry>, Option<SharedClock>),
            _: (Option<Msg>, Option<SessionId>, Option<ConfigError>, Option<StartupError>),
        ) {
        }
        let consts = [BIND_ENV, CONFIG_ENV, DEFAULT_CONFIG_PATH, DEFAULT_BIND];
        assert!(consts.iter().all(|c| !c.is_empty()));
        assert_eq!(EXIT_CONFIG, 78);
        assert_eq!(ControlAction::Clear, ControlAction::Clear);
        assert_ne!(ClockStatus::Ok, ClockStatus::Stale);
        assert_ne!(GroupSignal::Interrupt, GroupSignal::Kill);
        assert!(matches!(ReadOutcome::Idle, ReadOutcome::Idle));
        assert!(matches!(Decision::GiveUp { attempt: 1 }, Decision::GiveUp { .. }));
    }
}
