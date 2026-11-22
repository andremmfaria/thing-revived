//! Infrastructure shared in shape with the controller crate: TOML
//! configuration, `log`-based logging and the `types` catalogue. The two crates keep identical
//! utils layouts and dependency pins (`serde`, `toml`, `log`, `env_logger`).

pub mod config;
pub mod logging;
pub mod types;
