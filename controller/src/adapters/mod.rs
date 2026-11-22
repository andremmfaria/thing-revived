//! Adapters: implementations of the ports on top of the real system
//! (sysfs, timedatectl, files, systemctl, evdev, BlueZ, wpa_supplicant).

pub mod bluez;
pub mod evdev_input;
pub mod json_settings;
pub mod systemctl;
pub mod sysfs_backlight;
pub mod timedatectl;
pub mod uinput;
pub mod unavailable_bluetooth;
pub mod wpa_ctrl;
pub mod ws_server;
