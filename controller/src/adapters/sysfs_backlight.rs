//! Screen brightness via the standard Linux kernel backlight sysfs ABI.
//! See docs/research/settings-menu-feasibility.md §5. Confirmed live on
//! this device: node name `backlight` (i.e. `/sys/class/backlight/backlight/`),
//! `max_brightness` = 160.
//!
//! Reported/accepted range is the RAW sysfs range (0..=max_brightness),
//! not normalized to 0-100 - the WS client is expected to read `max` once
//! (from the `brightness` message) and scale its own UI, exactly the way
//! the kernel's own ABI exposes it (`docs.kernel.org/gpu/backlight.html`).
//! Normalizing here would silently hide the real max from the client and
//! buy nothing, since 160 is already a small, easy-to-scale number.

use crate::app::error::AppError;
use std::path::Path;

pub const BRIGHTNESS_PATH: &str = "/sys/class/backlight/backlight/brightness";
pub const MAX_BRIGHTNESS_PATH: &str = "/sys/class/backlight/backlight/max_brightness";

pub fn read_max() -> Result<u32, AppError> {
    read_u32_file(MAX_BRIGHTNESS_PATH)
}

pub fn read_current() -> Result<u32, AppError> {
    read_u32_file(BRIGHTNESS_PATH)
}

/// Validates `value` against the live `max_brightness` before writing -
/// never blindly passes an out-of-range value to the kernel attribute.
pub fn set(value: u32) -> Result<(), AppError> {
    validate(value, read_max()?)?;
    std::fs::write(BRIGHTNESS_PATH, value.to_string())
        .map_err(|e| AppError::failed(format!("write {BRIGHTNESS_PATH}: {e}")))
}

/// Pure range check, split out from `set()` so it has a unit test with no
/// dependency on a real sysfs node (this repo's test host has none).
pub fn validate(value: u32, max: u32) -> Result<(), AppError> {
    if value > max {
        Err(AppError::invalid(format!("brightness {value} out of range 0..={max}")))
    } else {
        Ok(())
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

    #[test]
    fn accepts_in_range_values() {
        assert!(validate(0, 160).is_ok());
        assert!(validate(160, 160).is_ok());
        assert!(validate(80, 160).is_ok());
    }

    #[test]
    fn out_of_range_is_an_invalid_error() {
        let e = validate(161, 160).unwrap_err();
        assert_eq!(e.kind, crate::app::error::ErrorKind::Invalid);
        assert_eq!(e.message, "brightness 161 out of range 0..=160");
    }

    #[test]
    fn rejects_out_of_range_values() {
        assert!(validate(161, 160).is_err());
        assert!(validate(u32::MAX, 160).is_err());
    }
}
