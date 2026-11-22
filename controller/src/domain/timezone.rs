//! Timezone name validation. An arbitrary client-supplied string must never
//! reach a spawned command unchecked, so it is compared against the real
//! `list-timezones` output first.

use crate::utils::types::AppError;

pub fn validate_timezone(tz: &str, valid: &[String]) -> Result<(), AppError> {
    if valid.iter().any(|z| z == tz) {
        Ok(())
    } else {
        Err(AppError::invalid(format!("'{tz}' is not a recognized IANA timezone")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_list() -> Vec<String> {
        vec!["Europe/Lisbon".into(), "America/New_York".into(), "UTC".into()]
    }

    #[test]
    fn accepts_a_real_zone_name() {
        assert!(validate_timezone("Europe/Lisbon", &fixture_list()).is_ok());
    }

    #[test]
    fn unknown_zone_is_an_invalid_error() {
        let e = validate_timezone("Not/AZone", &fixture_list()).unwrap_err();
        assert_eq!(e.kind, crate::utils::types::ErrorKind::Invalid);
    }

    #[test]
    fn rejects_an_unknown_string() {
        assert!(validate_timezone("Not/AZone", &fixture_list()).is_err());
    }

    #[test]
    fn rejects_shell_metacharacters_as_simply_unknown() {
        // Not a shell-injection risk either way (Command::args, no shell
        // interpolation) - but confirms garbage input is rejected by the
        // validator rather than silently passed through to the subprocess.
        assert!(validate_timezone("UTC; rm -rf /", &fixture_list()).is_err());
    }
}
