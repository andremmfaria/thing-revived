//! TOML configuration for controller-agent.
//!
//! Every section and key has a compiled-in default equal to the value that
//! used to be hard-coded, so a missing file or a partial file is fine. The
//! documented example is `config/controller.toml` in the repo (installed as
//! `/etc/thing-revived/controller.toml`); a test keeps it equal to
//! `Config::default()`.
//!
//! Anything that fails to read, parse or validate is a `ConfigError`, which
//! `main` turns into a message on stderr and exit code 78 (EX_CONFIG).

use serde::Deserialize;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Env var naming the config file.
pub const CONFIG_ENV: &str = "CONTROLLER_CONFIG";
/// Where the build installs the config on the device.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/thing-revived/controller.toml";
/// Exit code for a bad configuration (sysexits EX_CONFIG).
pub const EX_CONFIG: i32 = 78;

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub input: InputConfig,
    pub bluetooth: BluetoothConfig,
    pub wifi: WifiConfig,
    pub display: DisplayConfig,
    pub timezone: TimezoneConfig,
    pub settings: SettingsConfig,
    pub users: UsersConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// `host:port` of the WebSocket listener. Loopback unless a developer
    /// deliberately widens it (env `CONTROLLER_BIND` wins over this).
    pub bind: String,
    /// Per-client outbound queue depth before a stalled client is dropped.
    pub client_queue_capacity: usize,
    /// Idle seconds before a WebSocket ping is sent to notice dead clients.
    pub ping_interval_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig { bind: "127.0.0.1:8765".into(), client_queue_capacity: 256, ping_interval_secs: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// One of error, warn, info, debug, trace. Env `RUST_LOG` overrides.
    pub level: String,
    /// Optional env_logger style filter, e.g. `controller_agent::wifi=debug`.
    /// Empty means none.
    pub filter: String,
    /// Prefix lines with seconds since the Unix epoch (journald already adds
    /// timestamps, so this is off by default).
    pub timestamps: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig { level: "info".into(), filter: String::new(), timestamps: false }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    /// evdev node of the six physical buttons (grabbed exclusively).
    pub buttons_device: String,
    /// evdev node of the rotary knob (left ungrabbed).
    pub knob_device: String,
    /// KEY_M hold time in milliseconds that triggers the restart.
    pub longpress_ms: u64,
    /// systemd unit restarted by the KEY_M long press.
    pub restart_unit: String,
    /// Attempts to open an input device at startup.
    pub open_retries: u32,
    /// Delay between those attempts, in milliseconds.
    pub open_retry_delay_ms: u64,
    /// Pause after an evdev read error before retrying, in milliseconds.
    pub read_error_delay_ms: u64,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            buttons_device: "/dev/input/event0".into(),
            knob_device: "/dev/input/event1".into(),
            longpress_ms: 1500,
            restart_unit: "thing-revived-terminal-agent.service".into(),
            open_retries: 10,
            open_retry_delay_ms: 500,
            read_error_delay_ms: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BluetoothConfig {
    /// Length of one discovery window, in seconds.
    pub scan_duration_secs: u64,
    /// How long a pairing confirmation waits for the client's answer.
    pub confirm_timeout_secs: u64,
    /// Seconds between BlueZ object polls while scanning.
    pub scan_poll_interval_secs: u64,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        BluetoothConfig { scan_duration_secs: 150, confirm_timeout_secs: 30, scan_poll_interval_secs: 2 }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WifiConfig {
    /// Seconds between adapter-presence and status polls.
    pub poll_interval_secs: u64,
    /// Minimum seconds between attempts to start the wpa_supplicant unit.
    pub service_start_retry_secs: u64,
    /// Seconds to wait for scan results.
    pub scan_wait_secs: u64,
    /// Seconds to wait for association and authentication.
    pub connect_timeout_secs: u64,
    /// Seconds to wait for an IP address after association.
    pub dhcp_timeout_secs: u64,
    /// Seconds to wait for a wpa_supplicant control-socket reply.
    pub ctrl_reply_timeout_secs: u64,
    /// Directory holding the wpa_supplicant control sockets (env
    /// `WPA_CTRL_DIR` overrides, used by tests).
    pub ctrl_dir: String,
    /// Root-only state directory on the data partition.
    pub state_dir: String,
    /// wpa_supplicant configuration file created on first run.
    pub conf_path: String,
    /// Where network interfaces are listed.
    pub net_class_dir: String,
}

impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            poll_interval_secs: 3,
            service_start_retry_secs: 10,
            scan_wait_secs: 15,
            connect_timeout_secs: 40,
            dhcp_timeout_secs: 25,
            ctrl_reply_timeout_secs: 5,
            ctrl_dir: "/run/wpa_supplicant".into(),
            state_dir: "/var/lib/thing-revived".into(),
            conf_path: "/var/lib/thing-revived/wpa_supplicant.conf".into(),
            net_class_dir: "/sys/class/net".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// sysfs brightness attribute (write).
    pub brightness_path: String,
    /// sysfs max_brightness attribute (read).
    pub max_brightness_path: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        DisplayConfig {
            brightness_path: "/sys/class/backlight/backlight/brightness".into(),
            max_brightness_path: "/sys/class/backlight/backlight/max_brightness".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimezoneConfig {
    /// Program used to list, read and set the timezone (looked up on PATH).
    pub command: String,
}

impl Default for TimezoneConfig {
    fn default() -> Self {
        TimezoneConfig { command: "timedatectl".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsConfig {
    /// JSON file persisting brightness and timezone across reboots.
    pub path: String,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        SettingsConfig { path: "/var/lib/thing-revived/settings.json".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsersConfig {
    /// Minimum time between the password reaching the screen and the second
    /// M press being accepted as the acknowledgement, in milliseconds. An
    /// input debounce against contact bounce and a double tap, not a display
    /// timeout: the password stays up until that second press.
    pub ack_min_gap_ms: u64,
    /// Directory holding the account `state.json` and the `kiosk.locked` flag.
    pub state_dir: String,
    /// Idle seconds after which an unlocked admin session ends.
    pub session_idle_secs: u64,
    /// Wrong unlock passwords per connection that cost no waiting.
    pub unlock_free_attempts: u32,
    /// Longest wait between unlock attempts on one connection, seconds.
    pub unlock_backoff_max_secs: u64,
    /// Wrong unlock passwords across all connections per window before a
    /// fresh physical knob push is required for each further attempt.
    pub unlock_global_cap: usize,
    pub unlock_global_window_secs: u64,
    /// Mutating commands allowed per connection per minute.
    pub mutations_per_minute: usize,
    pub max_users: usize,
    pub max_groups: usize,
    pub max_keys_per_user: usize,
    /// SHA-512 crypt rounds for `chpasswd`; must match `common-password`.
    pub sha_rounds: u32,
    /// Parent directory of the homes the menu creates.
    pub home_base: String,
    /// Directory of the menu-managed `authorized_keys` files (sshd's
    /// `AuthorizedKeysFile`).
    pub keys_dir: String,
    /// System groups besides managed ones the menu may assign.
    pub assignable_system_groups: Vec<String>,
    /// Seconds before a shadow-utils or ssh-keygen call is killed.
    pub tool_timeout_secs: u64,
}

impl Default for UsersConfig {
    fn default() -> Self {
        UsersConfig {
            ack_min_gap_ms: 1500,
            state_dir: "/var/lib/thing-revived/accounts".into(),
            session_idle_secs: 300,
            unlock_free_attempts: 3,
            unlock_backoff_max_secs: 300,
            unlock_global_cap: 20,
            unlock_global_window_secs: 600,
            mutations_per_minute: 10,
            max_users: 16,
            max_groups: 32,
            max_keys_per_user: 20,
            sha_rounds: 65536,
            home_base: "/var/home".into(),
            keys_dir: "/var/lib/thing-revived-ssh/authorized_keys".into(),
            assignable_system_groups: vec!["sudo".into(), "adm".into()],
            tool_timeout_secs: 10,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ConfigError {
    Read { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, message } => write!(f, "cannot read config {}: {message}", path.display()),
            ConfigError::Parse { path, message } => write!(f, "invalid TOML in {}: {message}", path.display()),
            ConfigError::Invalid(m) => write!(f, "invalid configuration: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Where the effective configuration came from, so `main` can log it once the
/// logger exists (the logger itself needs the config).
#[derive(Debug, PartialEq)]
pub enum Source {
    File(PathBuf),
    /// The file did not exist; compiled-in defaults are in effect.
    Defaults(PathBuf),
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub source: Source,
}

const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

impl Config {
    /// Parses and validates a TOML document. `path` is only for messages.
    pub fn from_toml(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text)
            .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string().trim_end().to_string() })?;
        config.validate()?;
        Ok(config)
    }

    /// Resolves the path from `CONTROLLER_CONFIG` (default
    /// `/etc/thing-revived/controller.toml`) and loads it.
    pub fn load() -> Result<Loaded, ConfigError> {
        let path = std::env::var_os(CONFIG_ENV).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
        Config::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Loaded, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Loaded { config: Config::from_toml(&text, path)?, source: Source::File(path.to_path_buf()) }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::default();
                config.validate()?;
                Ok(Loaded { config, source: Source::Defaults(path.to_path_buf()) })
            }
            Err(e) => Err(ConfigError::Read { path: path.to_path_buf(), message: e.to_string() }),
        }
    }

    /// The configured listen address (before the `CONTROLLER_BIND` override).
    pub fn bind_addr(&self) -> SocketAddr {
        // `validate` guarantees this parses; fall back to the loopback
        // default rather than panic if someone skipped validation.
        self.server.bind.trim().parse().unwrap_or_else(|_| ServerConfig::default().bind.parse().expect("default bind is valid"))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| Err(ConfigError::Invalid(m));
        let range = |name: &str, v: u64, lo: u64, hi: u64| -> Result<(), ConfigError> {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(ConfigError::Invalid(format!("{name} = {v} is out of range {lo}..={hi}")))
            }
        };
        let path_set = |name: &str, v: &str| -> Result<(), ConfigError> {
            if v.trim().is_empty() {
                Err(ConfigError::Invalid(format!("{name} must not be empty")))
            } else if v.contains('\0') {
                Err(ConfigError::Invalid(format!("{name} contains a NUL byte")))
            } else {
                Ok(())
            }
        };

        let s = &self.server;
        if let Err(e) = s.bind.trim().parse::<SocketAddr>() {
            return bad(format!("server.bind = {:?} is not a valid host:port ({e})", s.bind));
        }
        range("server.client_queue_capacity", s.client_queue_capacity as u64, 1, 65_536)?;
        range("server.ping_interval_secs", s.ping_interval_secs, 1, 3_600)?;

        let l = &self.logging;
        if !LEVELS.contains(&l.level.as_str()) {
            return bad(format!("logging.level = {:?} must be one of {}", l.level, LEVELS.join(", ")));
        }
        if l.filter.chars().any(|c| c.is_control()) {
            return bad("logging.filter contains control characters".into());
        }

        let i = &self.input;
        path_set("input.buttons_device", &i.buttons_device)?;
        path_set("input.knob_device", &i.knob_device)?;
        range("input.longpress_ms", i.longpress_ms, 100, 60_000)?;
        path_set("input.restart_unit", &i.restart_unit)?;
        if i.restart_unit.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return bad("input.restart_unit must not contain whitespace".into());
        }
        range("input.open_retries", u64::from(i.open_retries), 1, 1_000)?;
        range("input.open_retry_delay_ms", i.open_retry_delay_ms, 1, 60_000)?;
        range("input.read_error_delay_ms", i.read_error_delay_ms, 1, 60_000)?;

        let b = &self.bluetooth;
        range("bluetooth.scan_duration_secs", b.scan_duration_secs, 1, 3_600)?;
        range("bluetooth.confirm_timeout_secs", b.confirm_timeout_secs, 1, 600)?;
        range("bluetooth.scan_poll_interval_secs", b.scan_poll_interval_secs, 1, 60)?;

        let w = &self.wifi;
        range("wifi.poll_interval_secs", w.poll_interval_secs, 1, 600)?;
        range("wifi.service_start_retry_secs", w.service_start_retry_secs, 1, 3_600)?;
        range("wifi.scan_wait_secs", w.scan_wait_secs, 1, 600)?;
        range("wifi.connect_timeout_secs", w.connect_timeout_secs, 1, 600)?;
        range("wifi.dhcp_timeout_secs", w.dhcp_timeout_secs, 1, 600)?;
        range("wifi.ctrl_reply_timeout_secs", w.ctrl_reply_timeout_secs, 1, 600)?;
        path_set("wifi.ctrl_dir", &w.ctrl_dir)?;
        path_set("wifi.state_dir", &w.state_dir)?;
        path_set("wifi.conf_path", &w.conf_path)?;
        path_set("wifi.net_class_dir", &w.net_class_dir)?;

        path_set("display.brightness_path", &self.display.brightness_path)?;
        path_set("display.max_brightness_path", &self.display.max_brightness_path)?;
        path_set("timezone.command", &self.timezone.command)?;
        path_set("settings.path", &self.settings.path)?;

        let u = &self.users;
        range("users.ack_min_gap_ms", u.ack_min_gap_ms, 500, 5_000)?;
        path_set("users.state_dir", &u.state_dir)?;
        range("users.session_idle_secs", u.session_idle_secs, 30, 3_600)?;
        range("users.unlock_free_attempts", u.unlock_free_attempts as u64, 1, 10)?;
        range("users.unlock_backoff_max_secs", u.unlock_backoff_max_secs, 5, 3_600)?;
        range("users.unlock_global_cap", u.unlock_global_cap as u64, 1, 1_000)?;
        range("users.unlock_global_window_secs", u.unlock_global_window_secs, 10, 86_400)?;
        range("users.mutations_per_minute", u.mutations_per_minute as u64, 1, 600)?;
        range("users.max_users", u.max_users as u64, 1, 1_000)?;
        range("users.max_groups", u.max_groups as u64, 1, 1_000)?;
        range("users.max_keys_per_user", u.max_keys_per_user as u64, 1, 100)?;
        range("users.sha_rounds", u.sha_rounds as u64, 1_000, 999_999_999)?;
        range("users.tool_timeout_secs", u.tool_timeout_secs, 1, 120)?;
        path_set("users.home_base", &u.home_base)?;
        path_set("users.keys_dir", &u.keys_dir)?;
        for g in &u.assignable_system_groups {
            if !crate::domain::accounts::names::is_valid_name(g) {
                return bad(format!("users.assignable_system_groups: {g:?} is not a valid group name"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::from_toml(text, Path::new("test.toml"))
    }

    #[test]
    fn defaults_equal_the_previously_hard_coded_behavior() {
        let c = Config::default();
        assert_eq!(c.server.bind, "127.0.0.1:8765");
        assert_eq!(c.server.client_queue_capacity, 256);
        assert_eq!(c.server.ping_interval_secs, 10);
        assert_eq!(c.logging.level, "info");
        assert_eq!(c.input.buttons_device, "/dev/input/event0");
        assert_eq!(c.input.knob_device, "/dev/input/event1");
        assert_eq!(c.input.longpress_ms, 1500);
        assert_eq!(c.input.restart_unit, "thing-revived-terminal-agent.service");
        assert_eq!((c.input.open_retries, c.input.open_retry_delay_ms, c.input.read_error_delay_ms), (10, 500, 200));
        assert_eq!(c.bluetooth.scan_duration_secs, 150);
        assert_eq!(c.bluetooth.confirm_timeout_secs, 30);
        assert_eq!(c.bluetooth.scan_poll_interval_secs, 2);
        assert_eq!(c.wifi.poll_interval_secs, 3);
        assert_eq!(c.wifi.service_start_retry_secs, 10);
        assert_eq!(c.wifi.scan_wait_secs, 15);
        assert_eq!(c.wifi.connect_timeout_secs, 40);
        assert_eq!(c.wifi.dhcp_timeout_secs, 25);
        assert_eq!(c.wifi.ctrl_reply_timeout_secs, 5);
        assert_eq!(c.wifi.ctrl_dir, "/run/wpa_supplicant");
        assert_eq!(c.wifi.state_dir, "/var/lib/thing-revived");
        assert_eq!(c.wifi.conf_path, "/var/lib/thing-revived/wpa_supplicant.conf");
        assert_eq!(c.wifi.net_class_dir, "/sys/class/net");
        assert_eq!(c.display.brightness_path, "/sys/class/backlight/backlight/brightness");
        assert_eq!(c.display.max_brightness_path, "/sys/class/backlight/backlight/max_brightness");
        assert_eq!(c.timezone.command, "timedatectl");
        assert_eq!(c.settings.path, "/var/lib/thing-revived/settings.json");
        assert_eq!(c.users.ack_min_gap_ms, 1500);
        assert_eq!(c.users.state_dir, "/var/lib/thing-revived/accounts");
        let u = &c.users;
        assert_eq!((u.session_idle_secs, u.unlock_free_attempts, u.unlock_backoff_max_secs), (300, 3, 300));
        assert_eq!((u.unlock_global_cap, u.unlock_global_window_secs, u.mutations_per_minute), (20, 600, 10));
        assert_eq!((u.max_users, u.max_groups, u.max_keys_per_user, u.sha_rounds, u.tool_timeout_secs), (16, 32, 20, 65536, 10));
        assert_eq!((u.home_base.as_str(), u.keys_dir.as_str()), ("/var/home", "/var/lib/thing-revived-ssh/authorized_keys"));
        assert_eq!(u.assignable_system_groups, vec!["sudo", "adm"]);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn example_file_in_the_repo_equals_the_defaults() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/controller.toml");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let parsed = Config::from_toml(&text, &path).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn empty_and_partial_documents_fall_back_to_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        let c = parse("[server]\nbind = \"127.0.0.1:9000\"\n").unwrap();
        assert_eq!(c.server.bind, "127.0.0.1:9000");
        assert_eq!(c.server.client_queue_capacity, 256);
        assert_eq!(c.input, InputConfig::default());
    }

    #[test]
    fn missing_file_yields_defaults_and_says_so() {
        let p = std::env::temp_dir().join(format!("controller-no-such-{}.toml", std::process::id()));
        let loaded = Config::load_from(&p).unwrap();
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.source, Source::Defaults(p));
    }

    #[test]
    fn unreadable_file_is_an_error_not_a_default() {
        // A directory exists but cannot be read as a file.
        let err = Config::load_from(&std::env::temp_dir()).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }), "{err}");
    }

    #[test]
    fn invalid_toml_and_unknown_keys_are_clear_errors() {
        let err = parse("[server\nbind = 1").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("test.toml"), "{err}");
        assert!(matches!(parse("[server]\nbind_adress = \"x\"\n").unwrap_err(), ConfigError::Parse { .. }));
        assert!(matches!(parse("[server]\nclient_queue_capacity = \"many\"\n").unwrap_err(), ConfigError::Parse { .. }));
    }

    #[test]
    fn validation_rejects_bad_values() {
        let cases = [
            "[server]\nbind = \"not an address\"\n",
            "[server]\nbind = \"127.0.0.1\"\n",
            "[server]\nclient_queue_capacity = 0\n",
            "[server]\nclient_queue_capacity = 1000000\n",
            "[server]\nping_interval_secs = 0\n",
            "[logging]\nlevel = \"verbose\"\n",
            "[input]\nlongpress_ms = 0\n",
            "[input]\nlongpress_ms = 999999\n",
            "[input]\nbuttons_device = \"\"\n",
            "[input]\nrestart_unit = \"two words\"\n",
            "[input]\nopen_retries = 0\n",
            "[bluetooth]\nscan_duration_secs = 0\n",
            "[bluetooth]\nconfirm_timeout_secs = 100000\n",
            "[wifi]\nconnect_timeout_secs = 0\n",
            "[wifi]\nctrl_dir = \"  \"\n",
            "[display]\nbrightness_path = \"\"\n",
            "[timezone]\ncommand = \"\"\n",
            "[settings]\npath = \"\"\n",
            "[users]\nack_min_gap_ms = 499\n",
            "[users]\nack_min_gap_ms = 5001\n",
            "[users]\nstate_dir = \"  \"\n",
            "[users]\nsession_idle_secs = 29\n",
            "[users]\nunlock_free_attempts = 0\n",
            "[users]\nunlock_global_cap = 0\n",
            "[users]\nmutations_per_minute = 0\n",
            "[users]\nmax_keys_per_user = 101\n",
            "[users]\nsha_rounds = 999\n",
            "[users]\ntool_timeout_secs = 0\n",
            "[users]\nhome_base = \"\"\n",
            "[users]\nkeys_dir = \"\"\n",
            "[users]\nassignable_system_groups = [\"Bad Name\"]\n",
        ];
        for text in cases {
            let err = parse(text).expect_err(text);
            assert!(matches!(err, ConfigError::Invalid(_)), "{text}: {err}");
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn valid_overrides_are_accepted() {
        let c = parse("[server]\nbind = \"0.0.0.0:8765\"\n[logging]\nlevel = \"debug\"\nfilter = \"controller_agent::wifi=trace\"\n[input]\nlongpress_ms = 2500\n").unwrap();
        assert_eq!(c.bind_addr().port(), 8765);
        assert!(!c.bind_addr().ip().is_loopback());
        assert_eq!(c.logging.level, "debug");
        assert_eq!(c.input.longpress_ms, 2500);
    }
}
