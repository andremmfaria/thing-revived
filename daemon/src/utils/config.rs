//! Typed configuration for the terminal agent, loaded from TOML.
//!
//! Every struct is `#[serde(default)]`, so a missing file or missing keys
//! fall back to the compiled-in defaults below (which equal the values that
//! used to be hard-coded). Unknown keys are rejected so a typo is a clear
//! startup error (exit code 78) instead of a silently ignored setting.
//!
//! Path: `TERMINAL_AGENT_CONFIG`, default `/etc/thing-revived/terminal.toml`.
//! `TERMINAL_AGENT_BIND` (see net.rs) takes priority over the file.
//! The committed example `config/terminal.toml` is tested to equal
//! `Config::default()`.

use super::types::ConfigError;
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

pub const CONFIG_ENV: &str = "TERMINAL_AGENT_CONFIG";
pub const DEFAULT_CONFIG_PATH: &str = "/etc/thing-revived/terminal.toml";
pub const DEFAULT_BIND: &str = "127.0.0.1:8766";
/// Exit code for configuration errors (EX_CONFIG).
pub const EXIT_CONFIG: i32 = 78;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub pty: PtyConfig,
    pub respawn: RespawnConfig,
    pub clock: ClockConfig,
    /// `[[session]]` tables. When none are given the four default shells apply.
    #[serde(rename = "session")]
    pub sessions: Vec<SessionConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// Tab name; `session-<index>` when omitted.
    pub name: Option<String>,
    /// Program and arguments, e.g. `["/usr/bin/bash", "-l"]`.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Listen address. `TERMINAL_AGENT_BIND` overrides it.
    pub bind: String,
    /// Messages queued per client before a stalled client is dropped.
    pub client_queue_capacity: usize,
    /// Upper bound on one blocked socket write to a client.
    pub client_write_timeout_secs: u64,
    /// Time a new connection has to complete the WebSocket handshake.
    pub handshake_timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// `off`, `error`, `warn`, `info`, `debug` or `trace`. `RUST_LOG` overrides it.
    pub level: String,
    /// Prefix lines with a unix timestamp (journald already stamps lines).
    pub timestamps: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PtyConfig {
    /// Initial window size; 0 leaves the kernel default (0x0) until the
    /// first `resize` message, which is the historical behavior.
    pub initial_cols: u16,
    pub initial_rows: u16,
    /// Bytes read from a PTY per `read(2)`.
    pub read_buffer_size: usize,
    /// How often an idle reader re-checks whether its generation is stale.
    pub reader_poll_ms: i32,
    /// TERM given to every session, unless the agent itself already runs
    /// with a TERM other than empty or `dumb`.
    pub term: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RespawnConfig {
    /// Trailing window in which involuntary exits are counted.
    pub window_secs: u64,
    /// Exits within the window that still get an automatic respawn.
    pub max: usize,
    /// Delay before the 1st, 2nd, 3rd... automatic respawn.
    pub delays_secs: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    /// `false` skips the clock-sync thread entirely.
    pub enabled: bool,
    /// Seconds between polls while the system clock is synchronized.
    pub interval_secs: u64,
    /// Poll delays while the clock is not yet synchronized or the status
    /// command fails (last value repeats).
    pub backoff_secs: Vec<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            pty: PtyConfig::default(),
            respawn: RespawnConfig::default(),
            clock: ClockConfig::default(),
            sessions: (1..=4)
                .map(|i| SessionConfig {
                    name: Some(format!("shell-{i}")),
                    command: vec!["/usr/bin/bash".to_string(), "-l".to_string()],
                })
                .collect(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: DEFAULT_BIND.to_string(),
            client_queue_capacity: 256,
            client_write_timeout_secs: 30,
            handshake_timeout_secs: 10,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig { level: "info".to_string(), timestamps: false }
    }
}

impl Default for PtyConfig {
    fn default() -> Self {
        PtyConfig { initial_cols: 0, initial_rows: 0, read_buffer_size: 4096, reader_poll_ms: 200, term: "xterm-256color".to_string() }
    }
}

impl Default for RespawnConfig {
    fn default() -> Self {
        RespawnConfig { window_secs: 30, max: 3, delays_secs: vec![0, 2, 5] }
    }
}

impl Default for ClockConfig {
    fn default() -> Self {
        ClockConfig {
            enabled: true,
            interval_secs: 60,
            backoff_secs: vec![10, 20, 40],
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, source } => write!(f, "cannot read config {}: {source}", path.display()),
            ConfigError::Parse { path, message } => write!(f, "invalid TOML in {}: {message}", path.display()),
            ConfigError::Invalid { path, message } => write!(f, "invalid config {}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Result of `load`: the config plus where it came from, so `main` can log
/// the outcome after the logger exists (loading itself happens before).
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub path: PathBuf,
    /// False when the file did not exist and defaults were used.
    pub from_file: bool,
}

impl Config {
    /// Parses and validates a TOML document.
    pub fn from_toml(raw: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(raw).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.sessions.is_empty() {
            return Err("no sessions defined".to_string());
        }
        if let Some(i) = self.sessions.iter().position(|s| s.command.is_empty()) {
            return Err(format!("session {i} has an empty command"));
        }
        if self.server.bind.trim().parse::<std::net::SocketAddr>().is_err() {
            return Err(format!("server.bind {:?} is not a valid ip:port address", self.server.bind));
        }
        if self.server.client_queue_capacity == 0 {
            return Err("server.client_queue_capacity must be at least 1".to_string());
        }
        if self.server.client_write_timeout_secs == 0 || self.server.handshake_timeout_secs == 0 {
            return Err("server timeouts must be at least 1 second".to_string());
        }
        if crate::utils::logging::parse_level(&self.logging.level).is_none() {
            return Err(format!(
                "logging.level {:?} is not one of off, error, warn, info, debug, trace",
                self.logging.level
            ));
        }
        if !(64..=1 << 20).contains(&self.pty.read_buffer_size) {
            return Err("pty.read_buffer_size must be between 64 and 1048576".to_string());
        }
        if self.pty.reader_poll_ms < 1 {
            return Err("pty.reader_poll_ms must be at least 1".to_string());
        }
        if self.pty.term.is_empty() {
            return Err("pty.term must not be empty".to_string());
        }
        if self.respawn.delays_secs.is_empty() {
            return Err("respawn.delays_secs must not be empty".to_string());
        }
        if self.respawn.window_secs == 0 {
            return Err("respawn.window_secs must be at least 1".to_string());
        }
        if self.clock.enabled {
            if self.clock.interval_secs == 0 {
                return Err("clock.interval_secs must be at least 1".to_string());
            }
            if self.clock.backoff_secs.is_empty() {
                return Err("clock.backoff_secs must not be empty".to_string());
            }
        }
        Ok(())
    }
}

/// Resolves the config path from `TERMINAL_AGENT_CONFIG` (blank counts as
/// unset) and loads it.
pub fn load() -> Result<Loaded, ConfigError> {
    let path = match std::env::var(CONFIG_ENV) {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => PathBuf::from(DEFAULT_CONFIG_PATH),
    };
    load_from(&path)
}

/// A missing file yields defaults (`from_file == false`); an unreadable
/// file, invalid TOML or invalid values are errors.
pub fn load_from(path: &Path) -> Result<Loaded, ConfigError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded { config: Config::default(), path: path.to_path_buf(), from_file: false });
        }
        Err(source) => return Err(ConfigError::Read { path: path.to_path_buf(), source }),
    };
    let config = toml::from_str::<Config>(&raw)
        .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string() })?;
    config
        .validate()
        .map_err(|message| ConfigError::Invalid { path: path.to_path_buf(), message })?;
    Ok(Loaded { config, path: path.to_path_buf(), from_file: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sessions_are_the_four_shells() {
        let c = Config::default();
        assert_eq!(c.sessions.len(), 4);
        for (i, s) in c.sessions.iter().enumerate() {
            assert_eq!(s.name.as_deref(), Some(format!("shell-{}", i + 1).as_str()));
            assert_eq!(s.command, ["/usr/bin/bash", "-l"]);
        }
    }

    /// Every default equals the constant it replaced in the old main.rs.
    #[test]
    fn defaults_equal_previous_hardcoded_constants() {
        let c = Config::default();
        assert_eq!(DEFAULT_CONFIG_PATH, "/etc/thing-revived/terminal.toml");
        assert_eq!(c.server.bind, "127.0.0.1:8766");
        assert_eq!(c.server.client_queue_capacity, 256);
        assert_eq!(c.server.client_write_timeout_secs, 30);
        assert_eq!(c.logging.level, "info");
        assert!(!c.logging.timestamps);
        assert_eq!((c.pty.initial_cols, c.pty.initial_rows), (0, 0));
        assert_eq!(c.pty.read_buffer_size, 4096);
        assert_eq!(c.pty.term, "xterm-256color");
        assert_eq!(c.pty.reader_poll_ms, 200);
        assert_eq!(c.respawn.window_secs, 30);
        assert_eq!(c.respawn.max, 3);
        assert_eq!(c.respawn.delays_secs, [0, 2, 5]);
        assert!(c.clock.enabled);
        assert_eq!(c.clock.interval_secs, 60);
        assert_eq!(c.clock.backoff_secs, [10, 20, 40]);
        c.validate().unwrap();
    }

    #[test]
    fn empty_document_is_all_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn partial_document_overrides_only_given_keys() {
        let c = Config::from_toml(
            "[server]\nbind = \"0.0.0.0:9000\"\n[clock]\nenabled = false\n[[session]]\nname = \"a\"\ncommand = [\"sh\"]\n",
        )
        .unwrap();
        assert_eq!(c.server.bind, "0.0.0.0:9000");
        assert_eq!(c.server.client_queue_capacity, 256);
        assert!(!c.clock.enabled);
        assert_eq!(c.sessions.len(), 1);
        assert_eq!(c.sessions[0].name.as_deref(), Some("a"));
    }

    #[test]
    fn bad_documents_are_errors_not_panics() {
        for bad in [
            "not toml [",
            "bogus_key = 1",
            "[server]\nbogus = 1",
            "[server]\nbind = \"nonsense\"",
            "[server]\nclient_queue_capacity = 0",
            "[server]\nhandshake_timeout_secs = 0",
            "[logging]\nlevel = \"loud\"",
            "[pty]\nread_buffer_size = 1",
            "[respawn]\ndelays_secs = []",
            "[clock]\ninterval_secs = 0",
            "[[session]]\nname = \"x\"",
            "[[session]]\ncommand = []",
            "[server]\nclient_queue_capacity = \"many\"",
        ] {
            assert!(Config::from_toml(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn missing_file_gives_defaults_and_bad_file_errors() {
        let dir = std::env::temp_dir().join(format!("ta-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = load_from(&dir.join("absent.toml")).unwrap();
        assert!(!missing.from_file);
        assert_eq!(missing.config, Config::default());

        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[server\n").unwrap();
        assert!(matches!(load_from(&bad), Err(ConfigError::Parse { .. })));

        let invalid = dir.join("invalid.toml");
        std::fs::write(&invalid, "[logging]\nlevel = \"x\"\n").unwrap();
        assert!(matches!(load_from(&invalid), Err(ConfigError::Invalid { .. })));

        let good = dir.join("good.toml");
        std::fs::write(&good, "[logging]\nlevel = \"debug\"\n").unwrap();
        let l = load_from(&good).unwrap();
        assert!(l.from_file);
        assert_eq!(l.config.logging.level, "debug");

        // A directory is "unreadable", not "missing".
        assert!(matches!(load_from(&dir), Err(ConfigError::Read { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn committed_example_equals_defaults() {
        let path = format!("{}/../config/terminal.toml", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(Config::from_toml(&raw).unwrap(), Config::default());
    }
}
