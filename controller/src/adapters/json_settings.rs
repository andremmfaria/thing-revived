//! Persistence for menu-driven settings that the OS does NOT reliably
//! restore on its own (see `domain::settings`), as a JSON file. The path is
//! `[settings] path` in controller.toml; `/var`, `/var/lib` and `/home` are
//! confirmed writable across reboots on this read-only-rootfs image
//! (docs/dashboard.md's real `adb reboot` test).

use crate::app::error::AppError;
use crate::domain::settings::Settings;
use crate::ports::SettingsStore;
use crate::utils::config::SettingsConfig;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// On-disk shape. A private mirror of `Settings` so the domain type carries
/// no serde dependency; the JSON keys are unchanged.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
struct SettingsFile {
    brightness: Option<u32>,
    timezone: Option<String>,
}

impl From<SettingsFile> for Settings {
    fn from(f: SettingsFile) -> Settings {
        Settings { brightness: f.brightness, timezone: f.timezone }
    }
}

impl From<&Settings> for SettingsFile {
    fn from(s: &Settings) -> SettingsFile {
        SettingsFile { brightness: s.brightness, timezone: s.timezone.clone() }
    }
}

pub struct JsonSettings {
    path: PathBuf,
}

impl JsonSettings {
    pub fn new(cfg: &SettingsConfig) -> JsonSettings {
        JsonSettings { path: PathBuf::from(&cfg.path) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore for JsonSettings {
    fn load(&self) -> Settings {
        load_from(&self.path)
    }

    fn save(&self, settings: &Settings) -> Result<(), AppError> {
        save_to(&self.path, settings).map_err(|e| AppError::failed(format!("write {}: {e}", self.path.display())))
    }
}

/// Returns empty `Settings` on any read/parse failure (missing file on first
/// boot, corrupt file, etc.) rather than panicking: a settings file problem
/// must never take down the daemon, which also owns the buttons and knob the
/// device is otherwise unusable without.
fn load_from(path: &Path) -> Settings {
    let shown = path.display();
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<SettingsFile>(&raw).map(Settings::from).unwrap_or_else(|e| {
            warn!("settings: {shown} failed to parse: {e}, using defaults");
            Settings::default()
        }),
        Err(e) => {
            // Normal on first boot - not a loud error.
            info!("settings: {shown} not readable ({e}), starting with defaults");
            Settings::default()
        }
    }
}

/// Atomic replace: write a sibling temp file (0600, same directory so the
/// rename stays on one filesystem), fsync it, rename over the target, then
/// fsync the directory. A power cut leaves either the old or the new file,
/// never a truncated one. A stale temp file from an earlier crash is simply
/// truncated and reused.
fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let json = serde_json::to_string_pretty(&SettingsFile::from(settings))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);

    let result = (|| {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        // `mode` only applies on creation; fix up a leftover with other bits.
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        // Persist the rename itself.
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_roundtrip_through_json() {
        let s = SettingsFile { brightness: Some(80), timezone: Some("Europe/Lisbon".into()) };
        let json = serde_json::to_string(&s).unwrap();
        let back: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_default_to_none() {
        let back: SettingsFile = serde_json::from_str("{}").unwrap();
        assert_eq!(Settings::from(back), Settings::default());
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("controller-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_then_load_roundtrips_with_0600_and_no_temp_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir("roundtrip");
        let path = dir.join("nested").join("settings.json");
        let s = Settings { brightness: Some(42), timezone: Some("Europe/Lisbon".into()) };
        save_to(&path, &s).unwrap();
        assert_eq!(load_from(&path), s);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!path.with_file_name("settings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftover_temp_file_and_existing_target_are_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir("leftover");
        let path = dir.join("settings.json");
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&path, r#"{"brightness":1,"timezone":null}"#).unwrap();
        std::fs::write(&tmp, "garbage from a crashed write that is longer than the new content ........................").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        let s = Settings { brightness: Some(7), timezone: None };
        save_to(&path, &s).unwrap();
        assert_eq!(load_from(&path), s);
        assert!(!tmp.exists());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_write_leaves_target_intact_and_corrupt_target_falls_back() {
        let dir = tempdir("corrupt");
        let path = dir.join("settings.json");
        let good = Settings { brightness: Some(9), timezone: Some("UTC".into()) };
        save_to(&path, &good).unwrap();
        // A crash mid-write only ever touches the temp file.
        std::fs::write(dir.join("settings.json.tmp"), "{\"brightn").unwrap();
        assert_eq!(load_from(&path), good);
        // Corrupt target: same fallback as before.
        std::fs::write(&path, "{\"brightn").unwrap();
        assert_eq!(load_from(&path), Settings::default());
        assert_eq!(load_from(&dir.join("missing.json")), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_roundtrip_and_save_errors_are_typed() {
        let dir = tempdir("port");
        let store = JsonSettings::new(&SettingsConfig { path: dir.join("s.json").to_string_lossy().into_owned() });
        assert_eq!(store.load(), Settings::default());
        let s = Settings { brightness: Some(3), timezone: Some("UTC".into()) };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
        // A path whose parent is a regular file cannot be created.
        let blocked = JsonSettings::new(&SettingsConfig { path: dir.join("s.json/inner/x.json").to_string_lossy().into_owned() });
        let e = blocked.save(&s).unwrap_err();
        assert_eq!(e.kind, crate::app::error::ErrorKind::Failed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
