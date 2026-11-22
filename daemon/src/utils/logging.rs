//! Logging setup: `log` facade, `env_logger` backend writing to stderr
//! (journald captures it).
//!
//! Format: `[terminal-agent] LEVEL target: message`, with a leading unix
//! timestamp (`secs.millis`) only when `[logging] timestamps = true`, since
//! journald stamps lines itself. The `RUST_LOG` environment variable
//! overrides the configured level.
//!
//! Privacy rule for the whole crate: PTY output bytes and user keystrokes
//! are never passed to a log macro at any level. Per-chunk messages, if
//! ever added, belong at `trace` and must still carry sizes only.

use crate::utils::config::LoggingConfig;
use log::LevelFilter;
use std::fmt::Display;
use std::io::Write;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Parses a configured level name (`off`, `error`, `warn`, `info`, `debug`, `trace`).
pub fn parse_level(name: &str) -> Option<LevelFilter> {
    LevelFilter::from_str(name.trim()).ok()
}

/// Renders one log line (without trailing newline).
pub fn format_line(level: log::Level, target: &str, msg: &dyn Display, timestamp: Option<(u64, u32)>) -> String {
    match timestamp {
        Some((secs, millis)) => format!("{secs}.{millis:03} [terminal-agent] {level} {target}: {msg}"),
        None => format!("[terminal-agent] {level} {target}: {msg}"),
    }
}

/// Installs the global logger. Safe to call more than once (later calls are
/// no-ops), which keeps tests that share a process simple.
pub fn init(cfg: &LoggingConfig) {
    let level = parse_level(&cfg.level).unwrap_or(LevelFilter::Info);
    let timestamps = cfg.timestamps;
    let mut builder = env_logger::Builder::new();
    builder.filter_level(level).parse_default_env();
    builder.format(move |buf, record| {
        let stamp = timestamps.then(|| {
            let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            (d.as_secs(), d.subsec_millis())
        });
        writeln!(buf, "{}", format_line(record.level(), record.target(), record.args(), stamp))
    });
    let _ = builder.try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format_without_timestamp() {
        let l = format_line(log::Level::Warn, "terminal_agent::server", &"hello", None);
        assert_eq!(l, "[terminal-agent] WARN terminal_agent::server: hello");
    }

    #[test]
    fn line_format_with_timestamp() {
        let l = format_line(log::Level::Info, "t", &"m", Some((1700000000, 5)));
        assert_eq!(l, "1700000000.005 [terminal-agent] INFO t: m");
    }

    #[test]
    fn levels_parse() {
        for (s, l) in [("off", LevelFilter::Off), ("error", LevelFilter::Error), ("Warn", LevelFilter::Warn), ("info", LevelFilter::Info), ("debug", LevelFilter::Debug), ("trace", LevelFilter::Trace)] {
            assert_eq!(parse_level(s), Some(l));
        }
        assert_eq!(parse_level("loud"), None);
        assert_eq!(parse_level(""), None);
    }
}
