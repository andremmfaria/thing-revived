//! Persisted device settings (mode, ssh, adb, uart): the value type with its
//! one invariant, the load, seed, reseed and fail-safe rules, the error
//! codes and the parsers for live tool output. Pure: adapters read and write
//! the files and run the tools, this module only interprets text.
//!
//! The invariant `mode == host implies adb == false` lives in the type:
//! the fields are private and every constructor, setter and load path goes
//! through `normalize`, so a `DeviceSettings` that breaks it cannot exist.
//!
//! JSON is read through `serde_json::Value` (the data model only, no derive
//! types) so the boot apply and the controller share one loader.

use crate::utils::types::AppError;
use serde_json::Value;

/// Compiled-in paths and unit names. The boot apply subcommands run before
/// `Config::load` and use these; a config test keeps the shipped
/// `[device_settings]` defaults equal to them.
pub const SETTINGS_PATH: &str = "/var/lib/thing-revived/device-settings.json";
pub const SEED_PATH: &str = "/usr/share/thing-revived/device-seed.json";
pub const LEGACY_BOOT_ROLE_PATH: &str = "/var/lib/superbird-usb-role/boot-role";
pub const SSH_OFF_FLAG_PATH: &str = "/run/thing-revived/ssh-off";
pub const SYSTEMCTL_COMMAND: &str = "systemctl";
pub const ROLE_COMMAND: &str = "superbird-usb-role";
pub const ADB_UNIT: &str = "android-tools-adbd.service";
pub const USB_BIND_UNIT: &str = "thing-revived-usb-bind.service";
pub const UART_UNIT: &str = "thing-revived-uart-root@ttyAML0.service";
pub const GETTY_UNIT: &str = "serial-getty@ttyAML0.service";
pub const SSH_SOCKET_UNIT: &str = "sshd.socket";
pub const SSH_INSTANCE_GLOB: &str = "sshd@*.service";

/// Schema version of both the settings file and the seed file.
pub const FILE_VERSION: u64 = 1;

/// The two roles the USB-C port can boot in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbRole {
    /// Gadget: the USB link to a PC (SSH, adb).
    Device,
    /// Host: USB Wi-Fi, Ethernet and storage adapters through a dock.
    Host,
}

impl UsbRole {
    pub fn as_str(self) -> &'static str {
        match self {
            UsbRole::Device => "device",
            UsbRole::Host => "host",
        }
    }

    /// Exact lowercase match only: the whitelist for client input.
    pub fn parse(s: &str) -> Option<UsbRole> {
        match s {
            "device" => Some(UsbRole::Device),
            "host" => Some(UsbRole::Host),
            _ => None,
        }
    }
}

/// What `superbird-usb-role status` reported. A role that is neither `host`
/// nor `device` reads as `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsbRoleStatus {
    /// The role the port is in right now (`role:` line).
    pub role: Option<UsbRole>,
    /// The persisted boot role (`boot role:` line). Cannot tell a missing
    /// file from an explicit device, so callers ignore it.
    pub boot_role: Option<UsbRole>,
}

/// Reads `role: ...` and `boot role: ...` lines; every other line and any
/// malformed line is ignored, a missing field is `None`.
pub fn parse_status(text: &str) -> UsbRoleStatus {
    let mut status = UsbRoleStatus::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = UsbRole::parse(&value.trim().to_ascii_lowercase());
        match key.trim().to_ascii_lowercase().as_str() {
            "role" => status.role = value,
            "boot role" => status.boot_role = value,
            _ => {}
        }
    }
    status
}

/// Reads the one line `systemctl is-active <unit>` prints. Only the exact
/// text `active` is on; anything else reads as off (fail safe).
pub fn parse_active(text: &str) -> bool {
    text.trim() == "active"
}

/// The four persisted settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    Mode,
    Ssh,
    Adb,
    Uart,
}

impl Setting {
    pub fn as_str(self) -> &'static str {
        match self {
            Setting::Mode => "mode",
            Setting::Ssh => "ssh",
            Setting::Adb => "adb",
            Setting::Uart => "uart",
        }
    }

    /// Exact lowercase match only: the whitelist for client input.
    pub fn parse(s: &str) -> Option<Setting> {
        match s {
            "mode" => Some(Setting::Mode),
            "ssh" => Some(Setting::Ssh),
            "adb" => Some(Setting::Adb),
            "uart" => Some(Setting::Uart),
            _ => None,
        }
    }

    /// `Setting::parse` with the wire error code.
    pub fn validate(s: &str) -> Result<Setting, ErrorCode> {
        Setting::parse(s).ok_or(ErrorCode::InvalidSetting)
    }
}

/// `on` or `off`, exact lowercase only.
pub fn parse_on_off(s: &str) -> Option<bool> {
    match s {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

pub fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

/// Stable wire-facing codes for every way a device settings command can be
/// refused. Callers compare these strings, never the free-text message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// No unlocked admin session on this connection.
    NotUnlocked,
    /// `setting` was not `mode`, `ssh`, `adb` or `uart`.
    InvalidSetting,
    /// `value` was not valid for the setting.
    InvalidValue,
    /// Enabling adb while the mode is host.
    AdbBlockedHost,
    /// Another mutation (or a reboot) is already running.
    Busy,
    /// Too many mutations on this connection in the window.
    RateLimited,
    /// The settings file could not be written.
    PersistFailed,
    /// The live change failed (unit start or stop, role switch).
    ApplyFailed,
    /// A tool could not be run at all.
    Unavailable,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotUnlocked => "not_unlocked",
            ErrorCode::InvalidSetting => "invalid_setting",
            ErrorCode::InvalidValue => "invalid_value",
            ErrorCode::AdbBlockedHost => "adb_blocked_host",
            ErrorCode::Busy => "busy",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::PersistFailed => "persist_failed",
            ErrorCode::ApplyFailed => "apply_failed",
            ErrorCode::Unavailable => "unavailable",
        }
    }
}

/// Maps an adapter failure to the code a caller reports: `Unavailable` keeps
/// its own code, everything else a live apply can fail with is `apply_failed`.
pub fn apply_error_code(e: &AppError) -> ErrorCode {
    match e.kind {
        crate::utils::types::ErrorKind::Unavailable => ErrorCode::Unavailable,
        _ => ErrorCode::ApplyFailed,
    }
}

/// The settings values. Fields are private so the invariant cannot be
/// bypassed: host mode never carries adb on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSettings {
    mode: UsbRole,
    ssh: bool,
    adb: bool,
    uart: bool,
}

impl DeviceSettings {
    /// Builds the value and applies the invariant (host forces adb off).
    pub fn new(mode: UsbRole, ssh: bool, adb: bool, uart: bool) -> DeviceSettings {
        DeviceSettings { mode, ssh, adb, uart }.normalized()
    }

    /// Fail-safe values for any doubt: device mode, ssh on (so a bad file
    /// cannot lock out remote access), adb and uart off.
    pub fn failsafe() -> DeviceSettings {
        DeviceSettings { mode: UsbRole::Device, ssh: true, adb: false, uart: false }
    }

    pub fn mode(&self) -> UsbRole {
        self.mode
    }

    pub fn ssh(&self) -> bool {
        self.ssh
    }

    pub fn adb(&self) -> bool {
        self.adb
    }

    pub fn uart(&self) -> bool {
        self.uart
    }

    /// Boolean settings only; `Mode` has no boolean (use `mode()`).
    pub fn flag(&self, setting: Setting) -> Option<bool> {
        match setting {
            Setting::Mode => None,
            Setting::Ssh => Some(self.ssh),
            Setting::Adb => Some(self.adb),
            Setting::Uart => Some(self.uart),
        }
    }

    /// The wire string of one setting (`host`/`device`, `on`/`off`).
    pub fn value_str(&self, setting: Setting) -> &'static str {
        match setting {
            Setting::Mode => self.mode.as_str(),
            Setting::Ssh => on_off(self.ssh),
            Setting::Adb => on_off(self.adb),
            Setting::Uart => on_off(self.uart),
        }
    }

    /// `Some("mode_host")` while the adb row must be greyed out.
    pub fn adb_blocked(&self) -> Option<&'static str> {
        (self.mode == UsbRole::Host).then_some("mode_host")
    }

    /// Setting the mode to host also turns adb off (in the same value, so
    /// one atomic write persists both).
    pub fn set_mode(&mut self, mode: UsbRole) {
        self.mode = mode;
        *self = self.normalized();
    }

    pub fn set_ssh(&mut self, on: bool) {
        self.ssh = on;
    }

    pub fn set_uart(&mut self, on: bool) {
        self.uart = on;
    }

    /// Turning adb off is always allowed; turning it on in host mode is
    /// refused with `adb_blocked_host` and leaves the value unchanged.
    pub fn set_adb(&mut self, on: bool) -> Result<(), ErrorCode> {
        if on && self.mode == UsbRole::Host {
            return Err(ErrorCode::AdbBlockedHost);
        }
        self.adb = on;
        Ok(())
    }

    /// Applies a client `value` string to one setting, on the same path the
    /// wire handler uses: whitelist parse, then the invariant.
    pub fn apply_value(&mut self, setting: Setting, value: &str) -> Result<(), ErrorCode> {
        match setting {
            Setting::Mode => {
                let mode = UsbRole::parse(value).ok_or(ErrorCode::InvalidValue)?;
                self.set_mode(mode);
                Ok(())
            }
            Setting::Ssh => {
                self.set_ssh(parse_on_off(value).ok_or(ErrorCode::InvalidValue)?);
                Ok(())
            }
            Setting::Adb => self.set_adb(parse_on_off(value).ok_or(ErrorCode::InvalidValue)?),
            Setting::Uart => {
                self.set_uart(parse_on_off(value).ok_or(ErrorCode::InvalidValue)?);
                Ok(())
            }
        }
    }

    fn normalized(mut self) -> DeviceSettings {
        if self.mode == UsbRole::Host {
            self.adb = false;
        }
        self
    }
}

/// The settings file text for `image_id` and `settings` (version 1, key
/// order as in the plan). Trailing newline included.
pub fn to_json(image_id: &str, s: &DeviceSettings) -> String {
    // Hand-written so the key order is fixed (serde_json's map sorts keys).
    let id = serde_json::to_string(image_id).unwrap_or_else(|_| "\"\"".into());
    format!(
        "{{\"version\":{FILE_VERSION},\"image_id\":{id},\"mode\":\"{}\",\"ssh\":{},\"adb\":{},\"uart\":{}}}\n",
        s.mode.as_str(),
        s.ssh,
        s.adb,
        s.uart
    )
}

/// The image baked seed (`/usr/share/thing-revived/device-seed.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seed {
    pub image_id: String,
    pub adb: bool,
    pub uart: bool,
}

/// Strict: valid JSON object, version 1, non-empty string `image_id`, bool
/// `adb` and `uart`. Anything else is no seed (`None`), never a guess.
pub fn parse_seed(text: &str) -> Option<Seed> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.as_object()?;
    if o.get("version")?.as_u64()? != FILE_VERSION {
        return None;
    }
    let image_id = o.get("image_id")?.as_str()?.to_string();
    if image_id.is_empty() {
        return None;
    }
    Some(Seed { image_id, adb: o.get("adb")?.as_bool()?, uart: o.get("uart")?.as_bool()? })
}

/// What a stored settings file held after parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Missing or non-string reads as `None` only when the key is absent;
    /// a wrong type makes the whole file corrupt.
    pub image_id: Option<String>,
    pub settings: DeviceSettings,
    /// True when the file said host with adb on and `settings` already has
    /// adb forced off.
    pub invariant_forced: bool,
}

/// Why a file was treated as corrupt. Never carries file content, so it is
/// safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corrupt {
    InvalidJson,
    NotAnObject,
    UnknownVersion,
    WrongType(&'static str),
    UnknownValue(&'static str),
}

impl Corrupt {
    pub fn reason(self) -> String {
        match self {
            Corrupt::InvalidJson => "invalid json".into(),
            Corrupt::NotAnObject => "not a json object".into(),
            Corrupt::UnknownVersion => "unknown or missing version".into(),
            Corrupt::WrongType(k) => format!("wrong type for {k}"),
            Corrupt::UnknownValue(k) => format!("unknown value for {k}"),
        }
    }
}

/// Parses a settings file. Unknown keys are ignored. A missing key falls
/// back individually (device, ssh on, adb off, uart off: a file exists, so
/// this is not first boot and nothing is inferred "on"). A wrong type, an
/// unknown value or version, or broken JSON makes the whole file corrupt.
pub fn parse_stored(text: &str) -> Result<Stored, Corrupt> {
    let v: Value = serde_json::from_str(text).map_err(|_| Corrupt::InvalidJson)?;
    let o = v.as_object().ok_or(Corrupt::NotAnObject)?;
    match o.get("version").and_then(Value::as_u64) {
        Some(FILE_VERSION) => {}
        _ => return Err(Corrupt::UnknownVersion),
    }
    let image_id = match o.get("image_id") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(Corrupt::WrongType("image_id")),
    };
    let flag = |key: &'static str, default: bool| -> Result<bool, Corrupt> {
        match o.get(key) {
            None => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(Corrupt::WrongType(key)),
        }
    };
    let mode = match o.get("mode") {
        None => UsbRole::Device,
        Some(Value::String(s)) => UsbRole::parse(s).ok_or(Corrupt::UnknownValue("mode"))?,
        Some(_) => return Err(Corrupt::WrongType("mode")),
    };
    let (ssh, adb, uart) = (flag("ssh", true)?, flag("adb", false)?, flag("uart", false)?);
    let settings = DeviceSettings::new(mode, ssh, adb, uart);
    Ok(Stored { image_id, settings, invariant_forced: mode == UsbRole::Host && adb })
}

/// The file state the adapter found at the settings path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredFile<'a> {
    /// No file (first boot).
    Missing,
    /// `/var` unreadable or the read failed for another reason than absence.
    Unreadable,
    Present(&'a str),
}

/// One thing `load` decided, for the journal. `message` carries no file
/// content, only ids and booleans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadNote {
    Seeded { image_id: String, adb: bool, uart: bool, mode: UsbRole },
    /// No file and no valid seed: fail-safe values with an empty image id.
    SeedInvalid,
    Reseeded { old: Option<String>, new: String, adb: bool, uart: bool, ssh: bool, mode: UsbRole },
    /// File valid but the seed is missing or invalid, so the image id could
    /// not be checked and nothing was reseeded.
    ImageIdUnchecked,
    Corrupt(Corrupt),
    InvariantForcedAdbOff,
    StoreUnreadable,
}

impl LoadNote {
    pub fn message(&self) -> String {
        match self {
            LoadNote::Seeded { image_id, adb, uart, mode } => {
                format!("device-settings: seeded image={image_id} mode={} ssh=true adb={adb} uart={uart}", mode.as_str())
            }
            LoadNote::SeedInvalid => "device-settings: no settings file and no valid seed, using fail-safe values".into(),
            LoadNote::Reseeded { old, new, adb, uart, ssh, mode } => format!(
                "device-settings: image id changed old={} new={new}, reseeded adb={adb} uart={uart}, kept ssh={ssh} mode={}",
                old.as_deref().unwrap_or("none"),
                mode.as_str()
            ),
            LoadNote::ImageIdUnchecked => "device-settings: seed missing or invalid, image id could not be checked, no reseed".into(),
            LoadNote::Corrupt(c) => format!("device-settings: settings file corrupt ({}), kept as .corrupt, using fail-safe values", c.reason()),
            LoadNote::InvariantForcedAdbOff => "device-settings: mode host with adb on, forcing adb off".into(),
            LoadNote::StoreUnreadable => "device-settings: settings store unreadable, using fail-safe values in memory, writes will fail".into(),
        }
    }

    /// Journal level: everything except a plain seed is a WARN.
    pub fn is_warn(&self) -> bool {
        !matches!(self, LoadNote::Seeded { .. })
    }
}

/// The result of the shared loader. The adapter performs the I/O it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub settings: DeviceSettings,
    /// The id to store (the seed id when known, else the stored one, else empty).
    pub image_id: String,
    /// Write `to_json(image_id, settings)` to the settings path now.
    pub write: bool,
    /// Rename the bad file to `device-settings.json.corrupt` (overwriting
    /// the previous one) before writing.
    pub quarantine: bool,
    /// False when the store is unreadable: every set must answer
    /// `persist_failed` and nothing must be written.
    pub persistable: bool,
    pub notes: Vec<LoadNote>,
}

/// Mode seed rule, used only when no settings file exists: ADB seed 1 means
/// device (an ADB build forces device today); otherwise a legacy
/// `boot-role` that reads exactly `host` or `device` (trimmed), else device.
pub fn seed_mode(seed_adb: bool, legacy_boot_role: Option<&str>) -> UsbRole {
    if seed_adb {
        return UsbRole::Device;
    }
    legacy_boot_role.and_then(|t| UsbRole::parse(t.trim())).unwrap_or(UsbRole::Device)
}

/// The shared loader (plan section 4.2 and 4.3). `seed` is the text of the
/// seed file if it could be read, `legacy_boot_role` the text of the legacy
/// boot-role file if it could be read. Pure: returns what to write and log.
pub fn load(stored: StoredFile<'_>, seed: Option<&str>, legacy_boot_role: Option<&str>) -> Loaded {
    let seed = seed.and_then(parse_seed);
    let seed_id = seed.as_ref().map(|s| s.image_id.clone()).unwrap_or_default();
    let mut notes = Vec::new();

    let failsafe = |notes: Vec<LoadNote>, write: bool, quarantine: bool, persistable: bool, image_id: String| Loaded {
        settings: DeviceSettings::failsafe(),
        image_id,
        write,
        quarantine,
        persistable,
        notes,
    };

    let text = match stored {
        StoredFile::Unreadable => {
            notes.push(LoadNote::StoreUnreadable);
            return failsafe(notes, false, false, false, seed_id);
        }
        StoredFile::Missing => {
            return match seed {
                Some(s) => {
                    let mode = seed_mode(s.adb, legacy_boot_role);
                    let settings = DeviceSettings::new(mode, true, s.adb, s.uart);
                    notes.push(LoadNote::Seeded { image_id: s.image_id.clone(), adb: settings.adb, uart: settings.uart, mode });
                    Loaded { settings, image_id: s.image_id, write: true, quarantine: false, persistable: true, notes }
                }
                None => {
                    notes.push(LoadNote::SeedInvalid);
                    failsafe(notes, true, false, true, String::new())
                }
            };
        }
        StoredFile::Present(t) => t,
    };

    let stored = match parse_stored(text) {
        Ok(s) => s,
        Err(c) => {
            notes.push(LoadNote::Corrupt(c));
            // The seed id (when known) is stored so the next boot does not
            // reseed adb back on over the fail-safe values.
            return failsafe(notes, true, true, true, seed_id);
        }
    };

    let mut write = false;
    let mut settings = stored.settings;
    let mut image_id = stored.image_id.clone().unwrap_or_default();
    match seed {
        None => notes.push(LoadNote::ImageIdUnchecked),
        Some(s) if stored.image_id.as_deref() != Some(s.image_id.as_str()) => {
            // adb and uart take the seed values; ssh and mode keep the
            // persisted ones; the invariant runs after the reset.
            settings = DeviceSettings::new(settings.mode(), settings.ssh(), s.adb, s.uart);
            notes.push(LoadNote::Reseeded {
                old: stored.image_id.clone(),
                new: s.image_id.clone(),
                adb: settings.adb,
                uart: settings.uart,
                ssh: settings.ssh,
                mode: settings.mode,
            });
            image_id = s.image_id;
            write = true;
        }
        Some(_) => {}
    }
    if stored.invariant_forced {
        notes.push(LoadNote::InvariantForcedAdbOff);
        write = true;
    }
    Loaded { settings, image_id, write, quarantine: false, persistable: true, notes }
}

/// Tolerant mode read for the failsafe unit: the persisted mode, device when
/// the file is missing, unreadable or corrupt. Never changes anything.
pub fn tolerant_mode(stored: StoredFile<'_>) -> UsbRole {
    match stored {
        StoredFile::Present(t) => parse_stored(t).map(|s| s.settings.mode()).unwrap_or(UsbRole::Device),
        _ => UsbRole::Device,
    }
}

/// Failsafe rule for the early unit failing: ssh stays reachable (key only
/// plus password with faillock, a lockout needs a factory reset), adb and
/// uart stay off because nothing starts them, boot-role is device.
pub const FAILSAFE_BOOT_ROLE: UsbRole = UsbRole::Device;
pub const FAILSAFE_SSH_REACHABLE: bool = true;

/// The `ssh-off` flag file exists exactly when persisted ssh is off.
pub fn ssh_off_flag_wanted(s: &DeviceSettings) -> bool {
    !s.ssh()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::types::ErrorKind;

    const SEED_OFF: &str = r#"{"version":1,"image_id":"img-2","adb":false,"uart":false}"#;
    const SEED_ON: &str = r#"{"version":1,"image_id":"img-2","adb":true,"uart":true}"#;

    fn stored(mode: &str, ssh: bool, adb: bool, uart: bool, id: &str) -> String {
        format!(r#"{{"version":1,"image_id":"{id}","mode":"{mode}","ssh":{ssh},"adb":{adb},"uart":{uart}}}"#)
    }

    fn s(mode: UsbRole, ssh: bool, adb: bool, uart: bool) -> DeviceSettings {
        DeviceSettings::new(mode, ssh, adb, uart)
    }

    // Row 1 of the 4.2 table: file missing, seed valid.
    #[test]
    fn row_missing_file_seeds_from_the_seed_file() {
        let l = load(StoredFile::Missing, Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Device, true, true, true));
        assert_eq!(l.image_id, "img-2");
        assert!(l.write && !l.quarantine && l.persistable);
        assert!(matches!(l.notes[0], LoadNote::Seeded { .. }));
        let l = load(StoredFile::Missing, Some(SEED_OFF), None);
        assert_eq!(l.settings, s(UsbRole::Device, true, false, false));
    }

    #[test]
    fn mode_seed_rule_honours_legacy_host_unless_adb_seed_is_on() {
        let l = load(StoredFile::Missing, Some(SEED_OFF), Some("host\n"));
        assert_eq!(l.settings.mode(), UsbRole::Host);
        let l = load(StoredFile::Missing, Some(SEED_ON), Some("host\n"));
        assert_eq!(l.settings.mode(), UsbRole::Device);
        assert!(l.settings.adb());
        for junk in ["", "Host", "otg", "host extra", "\n"] {
            let l = load(StoredFile::Missing, Some(SEED_OFF), Some(junk));
            assert_eq!(l.settings.mode(), UsbRole::Device, "{junk:?}");
        }
        assert_eq!(load(StoredFile::Missing, Some(SEED_OFF), Some(" device \n")).settings.mode(), UsbRole::Device);
    }

    // Row 2: seed also missing or invalid.
    #[test]
    fn row_missing_file_and_bad_seed_is_failsafe_with_empty_id() {
        let bad_seeds = [
            None,
            Some(""),
            Some("not json"),
            Some(r#"{"version":2,"image_id":"x","adb":true,"uart":true}"#),
            Some(r#"{"version":1,"image_id":"","adb":true,"uart":true}"#),
            Some(r#"{"version":1,"image_id":"x","adb":"yes","uart":true}"#),
            Some(r#"{"version":1,"image_id":"x","adb":true}"#),
            Some("[]"),
        ];
        for seed in bad_seeds {
            let l = load(StoredFile::Missing, seed, Some("host"));
            assert_eq!(l.settings, DeviceSettings::failsafe(), "{seed:?}");
            assert_eq!(l.image_id, "");
            assert!(l.write && l.persistable);
            assert_eq!(l.notes, vec![LoadNote::SeedInvalid]);
        }
    }

    // Row 3: file valid, image id equal.
    #[test]
    fn row_same_image_id_uses_the_stored_values() {
        let text = stored("host", false, false, true, "img-2");
        let l = load(StoredFile::Present(&text), Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Host, false, false, true));
        assert!(!l.write && !l.quarantine && l.notes.is_empty());
        assert_eq!(l.image_id, "img-2");
    }

    #[test]
    fn row_same_image_id_missing_keys_fall_back_individually() {
        // Not the seed values: a file exists so this is not first boot.
        let text = r#"{"version":1,"image_id":"img-2"}"#;
        let l = load(StoredFile::Present(text), Some(SEED_ON), None);
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert!(!l.write);
        let text = r#"{"version":1,"image_id":"img-2","uart":true,"extra":"ignored"}"#;
        let l = load(StoredFile::Present(text), Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Device, true, false, true));
    }

    // Row 4: image id differs or is missing, seed valid.
    #[test]
    fn row_image_change_reseeds_adb_and_uart_and_keeps_ssh_and_mode() {
        let text = stored("host", false, true, true, "img-1");
        let l = load(StoredFile::Present(&text), Some(SEED_OFF), None);
        assert_eq!(l.settings, s(UsbRole::Host, false, false, false));
        assert_eq!(l.image_id, "img-2");
        assert!(l.write);
        assert!(matches!(&l.notes[0], LoadNote::Reseeded { old: Some(o), new, .. } if o == "img-1" && new == "img-2"));
        assert!(l.notes[0].is_warn());
        let m = l.notes[0].message();
        assert!(m.contains("old=img-1") && m.contains("new=img-2") && m.contains("kept ssh=false mode=host"), "{m}");
    }

    #[test]
    fn reseed_to_adb_on_seed_while_host_forces_adb_off() {
        let text = stored("host", true, false, false, "img-1");
        let l = load(StoredFile::Present(&text), Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Host, true, false, true));
        assert!(l.write);
    }

    #[test]
    fn reseed_to_adb_on_seed_in_device_mode_turns_adb_on() {
        let text = stored("device", true, false, false, "img-1");
        let l = load(StoredFile::Present(&text), Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Device, true, true, true));
    }

    #[test]
    fn reseed_fires_when_the_stored_id_is_missing_and_is_idempotent() {
        let text = r#"{"version":1,"mode":"device","ssh":false,"adb":true,"uart":true}"#;
        let l = load(StoredFile::Present(text), Some(SEED_OFF), None);
        assert_eq!(l.settings, s(UsbRole::Device, false, false, false));
        assert!(matches!(&l.notes[0], LoadNote::Reseeded { old: None, .. }));
        assert!(l.notes[0].message().contains("old=none"));
        // Loading what it wrote does not reseed again.
        let written = to_json(&l.image_id, &l.settings);
        let again = load(StoredFile::Present(&written), Some(SEED_OFF), None);
        assert_eq!(again.settings, l.settings);
        assert!(!again.write && again.notes.is_empty());
    }

    // Row 5: file valid, seed missing or invalid.
    #[test]
    fn row_seed_unavailable_uses_values_without_reseed() {
        let text = stored("device", true, true, true, "img-1");
        for seed in [None, Some("garbage")] {
            let l = load(StoredFile::Present(&text), seed, None);
            assert_eq!(l.settings, s(UsbRole::Device, true, true, true));
            assert_eq!(l.image_id, "img-1");
            assert!(!l.write);
            assert_eq!(l.notes, vec![LoadNote::ImageIdUnchecked]);
        }
    }

    // Row 6: wrong type, unknown value, unknown version, invalid or truncated JSON.
    #[test]
    fn row_corrupt_file_is_quarantined_and_failsafe() {
        let good = stored("device", true, true, true, "img-2");
        let cases: Vec<(String, Corrupt)> = vec![
            ("".into(), Corrupt::InvalidJson),
            ("{".into(), Corrupt::InvalidJson),
            (good[..good.len() - 10].to_string(), Corrupt::InvalidJson),
            ("[]".into(), Corrupt::NotAnObject),
            (r#"{"version":2,"mode":"device"}"#.into(), Corrupt::UnknownVersion),
            (r#"{"mode":"device"}"#.into(), Corrupt::UnknownVersion),
            (r#"{"version":"1"}"#.into(), Corrupt::UnknownVersion),
            (r#"{"version":1,"ssh":"on"}"#.into(), Corrupt::WrongType("ssh")),
            (r#"{"version":1,"adb":1}"#.into(), Corrupt::WrongType("adb")),
            (r#"{"version":1,"uart":null}"#.into(), Corrupt::WrongType("uart")),
            (r#"{"version":1,"mode":true}"#.into(), Corrupt::WrongType("mode")),
            (r#"{"version":1,"image_id":5}"#.into(), Corrupt::WrongType("image_id")),
            (r#"{"version":1,"mode":"otg"}"#.into(), Corrupt::UnknownValue("mode")),
            (r#"{"version":1,"mode":"Host"}"#.into(), Corrupt::UnknownValue("mode")),
        ];
        for (text, why) in cases {
            let l = load(StoredFile::Present(&text), Some(SEED_ON), None);
            assert_eq!(l.settings, DeviceSettings::failsafe(), "{text}");
            assert!(l.write && l.quarantine && l.persistable, "{text}");
            assert_eq!(l.notes, vec![LoadNote::Corrupt(why)], "{text}");
            // The seed id is stored so the next boot does not reseed adb on.
            assert_eq!(l.image_id, "img-2");
            assert!(!l.notes[0].message().contains(&text) || text.is_empty());
        }
        let l = load(StoredFile::Present("{"), None, None);
        assert_eq!(l.image_id, "");
    }

    // Row 7: host and adb on.
    #[test]
    fn row_host_with_adb_on_forces_adb_off_and_writes() {
        let text = stored("host", true, true, true, "img-2");
        let l = load(StoredFile::Present(&text), Some(SEED_ON), None);
        assert_eq!(l.settings, s(UsbRole::Host, true, false, true));
        assert!(l.write);
        assert_eq!(l.notes, vec![LoadNote::InvariantForcedAdbOff]);
    }

    // Row 8: /var unreadable or read only.
    #[test]
    fn row_unreadable_store_is_failsafe_in_memory_and_not_persistable() {
        let l = load(StoredFile::Unreadable, Some(SEED_ON), Some("host"));
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert!(!l.write && !l.quarantine && !l.persistable);
        assert_eq!(l.notes, vec![LoadNote::StoreUnreadable]);
    }

    #[test]
    fn invariant_holds_in_the_type() {
        assert!(!DeviceSettings::new(UsbRole::Host, true, true, true).adb());
        let mut d = DeviceSettings::new(UsbRole::Device, true, true, false);
        assert!(d.adb());
        d.set_mode(UsbRole::Host);
        assert!(!d.adb());
        assert_eq!(d.adb_blocked(), Some("mode_host"));
        assert_eq!(d.set_adb(true), Err(ErrorCode::AdbBlockedHost));
        assert!(!d.adb());
        assert_eq!(d.set_adb(false), Ok(()));
        d.set_mode(UsbRole::Device);
        assert_eq!(d.adb_blocked(), None);
        assert!(!d.adb(), "returning to device leaves adb off");
        assert_eq!(d.set_adb(true), Ok(()));
        assert!(d.adb());
    }

    #[test]
    fn apply_value_is_a_strict_whitelist() {
        let mut d = DeviceSettings::failsafe();
        assert_eq!(d.apply_value(Setting::Mode, "host"), Ok(()));
        assert_eq!(d.apply_value(Setting::Adb, "on"), Err(ErrorCode::AdbBlockedHost));
        assert_eq!(d.apply_value(Setting::Ssh, "off"), Ok(()));
        assert_eq!(d.apply_value(Setting::Uart, "on"), Ok(()));
        assert_eq!(d, s(UsbRole::Host, false, false, true));
        for (setting, bad) in [
            (Setting::Mode, "on"),
            (Setting::Mode, "Host"),
            (Setting::Ssh, "true"),
            (Setting::Ssh, "host"),
            (Setting::Adb, "ON"),
            (Setting::Uart, ""),
            (Setting::Uart, "on "),
        ] {
            let before = d;
            assert_eq!(d.apply_value(setting, bad), Err(ErrorCode::InvalidValue), "{bad:?}");
            assert_eq!(d, before);
        }
        assert_eq!(d.value_str(Setting::Mode), "host");
        assert_eq!(d.value_str(Setting::Ssh), "off");
        assert_eq!(d.flag(Setting::Mode), None);
        assert_eq!(d.flag(Setting::Uart), Some(true));
    }

    #[test]
    fn setting_parse_is_a_strict_whitelist() {
        for (t, v) in [("mode", Setting::Mode), ("ssh", Setting::Ssh), ("adb", Setting::Adb), ("uart", Setting::Uart)] {
            assert_eq!(Setting::parse(t), Some(v));
            assert_eq!(v.as_str(), t);
        }
        for bad in ["", "Mode", "usb", "ssh ", "adb;reboot", "--adb"] {
            assert_eq!(Setting::validate(bad), Err(ErrorCode::InvalidSetting), "{bad:?}");
        }
    }

    #[test]
    fn json_round_trips_and_has_the_plan_shape() {
        let d = s(UsbRole::Host, false, false, true);
        let text = to_json("20261124T101500Z-c098ac5a", &d);
        assert_eq!(text, "{\"version\":1,\"image_id\":\"20261124T101500Z-c098ac5a\",\"mode\":\"host\",\"ssh\":false,\"adb\":false,\"uart\":true}\n");
        let back = parse_stored(&text).unwrap();
        assert_eq!(back.settings, d);
        assert_eq!(back.image_id.as_deref(), Some("20261124T101500Z-c098ac5a"));
        assert!(!back.invariant_forced);
    }

    #[test]
    fn tolerant_mode_defaults_to_device() {
        let host = stored("host", true, false, false, "x");
        assert_eq!(tolerant_mode(StoredFile::Present(&host)), UsbRole::Host);
        assert_eq!(tolerant_mode(StoredFile::Present("{")), UsbRole::Device);
        assert_eq!(tolerant_mode(StoredFile::Missing), UsbRole::Device);
        assert_eq!(tolerant_mode(StoredFile::Unreadable), UsbRole::Device);
    }

    #[test]
    fn failsafe_rules() {
        let f = DeviceSettings::failsafe();
        assert_eq!((f.mode(), f.ssh(), f.adb(), f.uart()), (UsbRole::Device, true, false, false));
        assert_eq!(FAILSAFE_BOOT_ROLE, UsbRole::Device);
        assert!(!ssh_off_flag_wanted(&f));
        assert!(ssh_off_flag_wanted(&s(UsbRole::Device, false, false, false)));
    }

    #[test]
    fn error_codes_are_the_wire_strings() {
        let all = [
            (ErrorCode::NotUnlocked, "not_unlocked"),
            (ErrorCode::InvalidSetting, "invalid_setting"),
            (ErrorCode::InvalidValue, "invalid_value"),
            (ErrorCode::AdbBlockedHost, "adb_blocked_host"),
            (ErrorCode::Busy, "busy"),
            (ErrorCode::RateLimited, "rate_limited"),
            (ErrorCode::PersistFailed, "persist_failed"),
            (ErrorCode::ApplyFailed, "apply_failed"),
            (ErrorCode::Unavailable, "unavailable"),
        ];
        for (c, t) in all {
            assert_eq!(c.as_str(), t);
        }
        assert_eq!(apply_error_code(&AppError::unavailable("x")), ErrorCode::Unavailable);
        assert_eq!(apply_error_code(&AppError::failed("x")), ErrorCode::ApplyFailed);
        assert_eq!(apply_error_code(&AppError::new(ErrorKind::Invalid, "x")), ErrorCode::ApplyFailed);
    }

    #[test]
    fn parse_active_only_trusts_the_exact_word() {
        assert!(parse_active("active\n"));
        for t in ["inactive", "failed", "activating", "unknown", "", "Active", "active active"] {
            assert!(!parse_active(t), "{t:?}");
        }
    }

    #[test]
    fn parses_the_documented_status_output() {
        let st = parse_status("role: device\nrevert: none\nboot role: host\n");
        assert_eq!(st, UsbRoleStatus { role: Some(UsbRole::Device), boot_role: Some(UsbRole::Host) });
        let st = parse_status("hello\n\n: : :\nrevert: none\nudc: ff400000.usb\nrole: host\nrole host\n");
        assert_eq!(st, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: None });
        assert_eq!(parse_status("role: otg\nboot role: unset\n"), UsbRoleStatus::default());
        let st = parse_status("  Role :  HOST  \r\nBoot Role: Device\r\n");
        assert_eq!(st, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: Some(UsbRole::Device) });
    }
}
