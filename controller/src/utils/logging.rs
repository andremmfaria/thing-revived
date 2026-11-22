//! Logging setup: `log` facade, `env_logger` backend on stderr (journald
//! captures it). Line format: `[controller] LEVEL target: message`, with no
//! timestamp unless `logging.timestamps` is set.

use crate::utils::config::LoggingConfig;
use log::{LevelFilter, Record};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Component prefix; the terminal-agent daemon uses `terminal-agent`.
pub const COMPONENT: &str = "controller";

/// Maps a validated level name to a filter; unknown names fall back to info.
pub fn level_filter(level: &str) -> LevelFilter {
    match level {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// Renders one record exactly as it is written to stderr (without the
/// trailing newline).
pub fn format_record(record: &Record<'_>, timestamps: bool) -> String {
    let mut line = String::new();
    if timestamps {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        line.push_str(&format!("{secs} "));
    }
    line.push_str(&format!("[{COMPONENT}] {} {}: {}", record.level(), record.target(), record.args()));
    line
}

/// Installs the global logger. `RUST_LOG`, when set, overrides the configured
/// level and filter. Safe to call once; a second call is ignored.
pub fn init(cfg: &LoggingConfig) {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(level_filter(&cfg.level));
    if !cfg.filter.trim().is_empty() {
        builder.parse_filters(cfg.filter.trim());
    }
    if let Ok(env) = std::env::var("RUST_LOG") {
        builder.parse_filters(&env);
    }
    let timestamps = cfg.timestamps;
    builder.format(move |buf, record| writeln!(buf, "{}", format_record(record, timestamps)));
    let _ = builder.try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(level: log::Level, target: &str, msg: std::fmt::Arguments<'_>, ts: bool) -> String {
        format_record(&Record::builder().level(level).target(target).args(msg).build(), ts)
    }

    #[test]
    fn format_is_component_level_target_message_without_timestamp() {
        let line = render(log::Level::Warn, "controller_agent::wifi", format_args!("interface gone"), false);
        assert_eq!(line, "[controller] WARN controller_agent::wifi: interface gone");
    }

    #[test]
    fn timestamps_are_opt_in_and_prefix_the_line() {
        let line = render(log::Level::Info, "t", format_args!("x"), true);
        let (secs, rest) = line.split_once(' ').unwrap();
        assert!(secs.parse::<u64>().is_ok(), "{line}");
        assert_eq!(rest, "[controller] INFO t: x");
    }

    #[test]
    fn level_names_map_to_filters() {
        assert_eq!(level_filter("error"), LevelFilter::Error);
        assert_eq!(level_filter("warn"), LevelFilter::Warn);
        assert_eq!(level_filter("info"), LevelFilter::Info);
        assert_eq!(level_filter("debug"), LevelFilter::Debug);
        assert_eq!(level_filter("trace"), LevelFilter::Trace);
    }

    #[test]
    fn a_secret_bearing_command_does_not_leak_through_the_log_path() {
        use crate::ClientCommand;
        let cmd: ClientCommand =
            serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        // The same formatting the router uses when it logs a command.
        let line = render(log::Level::Debug, "controller_agent::app", format_args!("client command: {cmd:?}"), false);
        assert!(!line.contains("hunter2"), "{line}");
        assert!(line.contains("<redacted>"), "{line}");
    }
}
