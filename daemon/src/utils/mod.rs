//! Infrastructure shared in shape with the controller crate: TOML
//! configuration and `log`-based logging. The two crates keep identical
//! utils layouts and dependency pins (`serde`, `toml`, `log`, `env_logger`).

pub mod config;
pub mod logging;
