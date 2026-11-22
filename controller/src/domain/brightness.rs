//! Brightness range check. Reported and accepted values are the RAW sysfs
//! range (0..=max_brightness, 160 on this device), not normalized to 0-100:
//! the client reads `max` once and scales its own UI, exactly as the kernel
//! ABI exposes it. Normalizing here would hide the real max for no gain.

use crate::app::error::AppError;

/// Pure range check, kept free of I/O so it has a unit test with no sysfs.
pub fn validate(value: u32, max: u32) -> Result<(), AppError> {
    if value > max {
        Err(AppError::invalid(format!("brightness {value} out of range 0..={max}")))
    } else {
        Ok(())
    }
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
