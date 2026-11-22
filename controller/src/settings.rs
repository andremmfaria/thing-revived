//! Persistence for menu-driven settings that the OS does NOT reliably
//! restore on its own. See docs/research/settings-menu-feasibility.md §8.
//!
//! Timezone needs this unconditionally (§7.2: `timedatectl set-timezone`'s
//! `/etc/localtime` symlink write doesn't survive a reboot on this
//! read-only-rootfs image). Brightness is more subtle: this BSP DOES ship
//! `systemd-backlight@backlight:backlight.service` (confirmed live,
//! `systemctl list-units`), which is systemd's own save/restore-on-boot
//! mechanism for exactly this sysfs value - but `/var/lib/systemd/backlight/`
//! was found empty on a live check (no saved-state file ever written), so
//! that mechanism's "save" half is unconfirmed to actually fire on this
//! image's shutdown/reboot path. Rather than rely on an unconfirmed OS
//! mechanism, this agent persists brightness itself too (belt-and-suspenders
//! with systemd-backlight, not a replacement for it) - see main.rs's startup
//! re-apply and bridge::backlight::set()'s post-write persistence.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// `/var`, `/var/lib`, and `/home` are confirmed writable across reboots on
/// this read-only-rootfs image (docs/dashboard.md's real `adb reboot` test);
/// `/var/lib/thing-revived/` is this project's own established convention
/// there (terminal-agent-config.json, the clock-sync SSH key).
pub const SETTINGS_PATH: &str = "/var/lib/thing-revived/settings.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Settings {
    /// Raw sysfs brightness value (0..=max_brightness, NOT normalized to
    /// 0-100 - see backlight.rs's doc comment for why).
    pub brightness: Option<u32>,
    /// An IANA timezone name, re-applied via `timedatectl set-timezone` at
    /// every startup rather than trusted to persist on its own (§7.2).
    pub timezone: Option<String>,
}

/// Returns an empty `Settings` on any read/parse failure (missing file on
/// first boot, corrupt file, etc.) rather than panicking - a settings file
/// problem must never take down the whole bridge daemon, which also owns
/// the buttons/knob the device is otherwise unusable without.
pub fn load_settings() -> Settings {
    load_from(Path::new(SETTINGS_PATH))
}

fn load_from(path: &Path) -> Settings {
    let shown = path.display();
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("[bridge] settings: {shown} failed to parse: {e}, using defaults");
            Settings::default()
        }),
        Err(e) => {
            // Normal on first boot - not a loud error.
            println!("[bridge] settings: {shown} not readable ({e}), starting with defaults");
            Settings::default()
        }
    }
}

pub fn save_settings(settings: &Settings) {
    if let Err(e) = save_to(Path::new(SETTINGS_PATH), settings) {
        eprintln!("[bridge] settings: failed to write {SETTINGS_PATH}: {e}");
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

    let json = serde_json::to_string_pretty(settings)
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
        let s = Settings { brightness: Some(80), timezone: Some("Europe/Lisbon".into()) };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_default_to_none() {
        let back: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Settings::default());
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
}
