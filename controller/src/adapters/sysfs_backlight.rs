//! Screen brightness via the standard Linux kernel backlight sysfs ABI. See
//! docs/research/settings-menu-feasibility.md section 5. Confirmed live on
//! this device: node `backlight` (`/sys/class/backlight/backlight/`),
//! `max_brightness` = 160. Paths come from `[display]` in controller.toml.

use crate::app::error::AppError;
use crate::domain::brightness::validate;
use crate::ports::Backlight;
use crate::utils::config::DisplayConfig;
use std::path::Path;

pub struct SysfsBacklight {
    brightness_path: String,
    max_brightness_path: String,
}

impl SysfsBacklight {
    pub fn new(cfg: &DisplayConfig) -> SysfsBacklight {
        SysfsBacklight { brightness_path: cfg.brightness_path.clone(), max_brightness_path: cfg.max_brightness_path.clone() }
    }
}

impl Backlight for SysfsBacklight {
    fn max(&self) -> Result<u32, AppError> {
        read_u32_file(&self.max_brightness_path)
    }

    fn get(&self) -> Result<u32, AppError> {
        read_u32_file(&self.brightness_path)
    }

    /// Validates `value` against the live `max_brightness` before writing,
    /// never blindly passing an out-of-range value to the kernel attribute.
    fn set(&self, value: u32) -> Result<(), AppError> {
        validate(value, self.max()?)?;
        std::fs::write(&self.brightness_path, value.to_string())
            .map_err(|e| AppError::failed(format!("write {}: {e}", self.brightness_path)))
    }
}

fn read_u32_file(path: &str) -> Result<u32, AppError> {
    if !Path::new(path).exists() {
        return Err(AppError::failed(format!("{path} does not exist")));
    }
    std::fs::read_to_string(path)
        .map_err(|e| AppError::failed(format!("read {path}: {e}")))?
        .trim()
        .parse::<u32>()
        .map_err(|e| AppError::failed(format!("parse {path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::error::ErrorKind;

    fn fake_sysfs(name: &str, max: &str, current: &str) -> (std::path::PathBuf, SysfsBacklight) {
        let dir = std::env::temp_dir().join(format!("controller-backlight-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_brightness"), max).unwrap();
        std::fs::write(dir.join("brightness"), current).unwrap();
        let cfg = DisplayConfig {
            brightness_path: dir.join("brightness").to_string_lossy().into_owned(),
            max_brightness_path: dir.join("max_brightness").to_string_lossy().into_owned(),
        };
        (dir, SysfsBacklight::new(&cfg))
    }

    #[test]
    fn reads_current_and_max_trimming_the_newline() {
        let (dir, b) = fake_sysfs("read", "160\n", "42\n");
        assert_eq!(b.max().unwrap(), 160);
        assert_eq!(b.get().unwrap(), 42);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_writes_in_range_values_only() {
        let (dir, b) = fake_sysfs("set", "160", "10");
        b.set(160).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("brightness")).unwrap(), "160");
        let e = b.set(161).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert_eq!(std::fs::read_to_string(dir.join("brightness")).unwrap(), "160", "rejected value must not be written");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_garbage_attributes_are_failed_errors() {
        let (dir, b) = fake_sysfs("bad", "not a number", "1");
        assert_eq!(b.max().unwrap_err().kind, ErrorKind::Failed);
        let missing = SysfsBacklight::new(&DisplayConfig {
            brightness_path: dir.join("nope").to_string_lossy().into_owned(),
            max_brightness_path: dir.join("nope2").to_string_lossy().into_owned(),
        });
        assert!(missing.get().unwrap_err().message.contains("does not exist"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
