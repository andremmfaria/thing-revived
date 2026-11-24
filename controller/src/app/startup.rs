//! Startup work that runs once before clients connect.

use crate::boot_apply::BootFiles;
use crate::domain::device_settings::Loaded;
use crate::ports::DeviceSettingsStore;
use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;

/// Re-applies the persisted brightness and timezone to the hardware, so a
/// reboot does not lose the user's settings. Missing values are skipped.
pub fn apply_persisted(display: &DisplayService, timezone: &TimezoneService) {
    display.apply_saved();
    timezone.apply_saved();
}

/// Loads the device settings for the service. The early unit normally has
/// done the load, seed, reseed and repair already and left its applied marker;
/// the load here is then a plain read. When the marker is absent (the unit
/// failed, was skipped, or the controller runs without it, for example a
/// patched binary without the new units), this is the fallback: the same
/// shared loader repairs the file, and the two files the early unit would have
/// written (`boot-role` mirror and the `ssh-off` flag) are written here too.
/// Nothing is started or stopped; the late unit owns live state.
pub fn load_device_settings(store: &dyn DeviceSettingsStore, files: &dyn BootFiles, early_unit_applied: bool) -> Loaded {
    let loaded = store.load();
    if early_unit_applied {
        return loaded;
    }
    log::warn!("device-settings: early unit marker missing, repairing from the controller");
    if let Err(e) = files.write_boot_role(loaded.settings.mode()) {
        log::warn!("device-settings: boot-role mirror failed: {e}");
    }
    if let Err(e) = files.set_ssh_off_flag(!loaded.settings.ssh()) {
        log::warn!("device-settings: ssh-off flag update failed: {e}");
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::device_settings::{DeviceSettings, UsbRole};
    use crate::utils::types::AppError;
    use std::sync::Mutex;

    struct Store(DeviceSettings);

    impl DeviceSettingsStore for Store {
        fn load(&self) -> Loaded {
            Loaded { settings: self.0, image_id: "i".into(), write: false, quarantine: false, persistable: true, notes: vec![] }
        }
        fn save(&self, _: &str, _: &DeviceSettings) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Files(Mutex<Vec<String>>);

    impl BootFiles for Files {
        fn write_boot_role(&self, role: UsbRole) -> Result<(), AppError> {
            self.0.lock().unwrap().push(format!("boot-role {}", role.as_str()));
            Ok(())
        }
        fn set_ssh_off_flag(&self, present: bool) -> Result<(), AppError> {
            self.0.lock().unwrap().push(format!("flag {present}"));
            Ok(())
        }
        fn mark_applied(&self) -> Result<(), AppError> {
            self.0.lock().unwrap().push("applied".into());
            Ok(())
        }
    }

    #[test]
    fn the_fallback_repairs_the_two_early_files_only_when_the_marker_is_missing() {
        let store = Store(DeviceSettings::new(UsbRole::Host, false, false, false));
        let files = Files::default();
        let loaded = load_device_settings(&store, &files, true);
        assert_eq!(loaded.settings.mode(), UsbRole::Host);
        assert!(files.0.lock().unwrap().is_empty(), "the early unit already did it");
        load_device_settings(&store, &files, false);
        assert_eq!(*files.0.lock().unwrap(), vec!["boot-role host", "flag true"], "no applied marker is faked");
    }
}
