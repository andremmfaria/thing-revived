//! Adapters: implementations of the ports on top of the real system
//! (sysfs, timedatectl, files, systemctl, evdev, BlueZ, wpa_supplicant).

pub mod accounts_db;
pub mod accounts_shadow;
pub mod authorized_keys_fs;
pub mod bluez;
pub mod debug_access_cli;
pub mod device_ctl_cli;
pub mod device_settings_json;
pub mod evdev_input;
pub mod json_settings;
pub mod os_entropy;
pub mod ssh_keygen;
pub mod systemctl;
pub mod sysfs_backlight;
pub mod timedatectl;
pub mod uinput;
pub mod usb_role_cli;
pub mod unavailable_bluetooth;
pub mod unix_chkpwd;
pub mod wpa_ctrl;
pub mod ws_server;
