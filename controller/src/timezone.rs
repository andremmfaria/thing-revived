//! Timezone get/set/list via `timedatectl`. See
//! docs/research/settings-menu-feasibility.md §7. Deliberately does NOT
//! add a "set system time" control - §7.2 reasons through why that would
//! be redundant with (and worse than) terminal/agent's existing SSH clock-
//! sync offset, and this file doesn't re-litigate that call.

use log::{error};
use std::process::Command;

/// Enumerates every valid IANA zone name `timedatectl set-timezone` will
/// accept. Called once at startup and cached by the caller (main.rs) -
/// this list changes only with a `tzdata` package update, never at
/// runtime, so re-shelling-out on every WS request would be pure overhead.
pub fn list_timezones() -> Vec<String> {
    match Command::new("timedatectl").arg("list-timezones").output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Ok(out) => {
            error!(
                "timedatectl list-timezones failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            Vec::new()
        }
        Err(e) => {
            error!("failed to spawn timedatectl list-timezones: {e}");
            Vec::new()
        }
    }
}

pub fn get_timezone() -> Result<String, String> {
    let out = Command::new("timedatectl")
        .args(["show", "--property=Timezone", "--value"])
        .output()
        .map_err(|e| format!("spawn timedatectl show: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "timedatectl show failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Validates `tz` against the real, live `list-timezones` output before
/// ever handing it to a shell-spawned command - an arbitrary WS-supplied
/// string must never reach `Command::new("timedatectl").arg(tz)` unchecked.
pub fn set_timezone(tz: &str, valid: &[String]) -> Result<(), String> {
    validate_timezone(tz, valid)?;
    let out = Command::new("timedatectl")
        .args(["set-timezone", tz])
        .output()
        .map_err(|e| format!("spawn timedatectl set-timezone: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "timedatectl set-timezone failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

pub fn validate_timezone(tz: &str, valid: &[String]) -> Result<(), String> {
    if valid.iter().any(|z| z == tz) {
        Ok(())
    } else {
        Err(format!("'{tz}' is not a recognized IANA timezone"))
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
