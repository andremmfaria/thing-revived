//! Adapters: implementations of the ports on top of the real system
//! (sysfs, timedatectl, files, systemctl, evdev, BlueZ, wpa_supplicant).

pub mod json_settings;
pub mod systemctl;
pub mod sysfs_backlight;
pub mod timedatectl;
