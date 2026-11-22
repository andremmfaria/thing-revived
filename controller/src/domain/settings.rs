//! The settings the OS does not reliably restore on its own: timezone
//! (`timedatectl set-timezone` does not survive a reboot on this read-only
//! rootfs image) and brightness (the stock systemd-backlight save half is
//! unconfirmed on this BSP, so the agent persists it itself too). Plain data;
//! the JSON file format belongs to `adapters::json_settings`.

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Settings {
    /// Raw sysfs brightness value (0..=max_brightness).
    pub brightness: Option<u32>,
    /// An IANA timezone name, re-applied at every startup.
    pub timezone: Option<String>,
}
