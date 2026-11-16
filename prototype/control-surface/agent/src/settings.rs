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
    match std::fs::read_to_string(SETTINGS_PATH) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("[bridge] settings: {SETTINGS_PATH} failed to parse: {e}, using defaults");
            Settings::default()
        }),
        Err(e) => {
            // Normal on first boot - not a loud error.
            println!("[bridge] settings: {SETTINGS_PATH} not readable ({e}), starting with defaults");
            Settings::default()
        }
    }
}

pub fn save_settings(settings: &Settings) {
    if let Some(parent) = Path::new(SETTINGS_PATH).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[bridge] settings: failed to create {parent:?}: {e}");
            return;
        }
    }
    match serde_json::to_string_pretty(settings) {
        Ok(json) => {
            if let Err(e) = std::fs::write(SETTINGS_PATH, json) {
                eprintln!("[bridge] settings: failed to write {SETTINGS_PATH}: {e}");
            }
        }
        Err(e) => eprintln!("[bridge] settings: failed to serialize: {e}"),
    }
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
}
