//! Where the clock state comes from: the system's own NTP synchronization
//! flag. `timedatectl show -p NTPSynchronized --value` prints `yes` once
//! systemd-timesyncd (or any NTP client that sets the kernel flag through
//! systemd) has synchronized the clock, `no` before. Parsing is pure.

use std::process::Command;

/// Why the clock state could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

/// Reads whether the system clock is NTP synchronized.
pub trait ClockSource {
    fn is_synchronized(&self) -> Result<bool, SourceError>;
}

/// Parses the `--value` output of `timedatectl show -p NTPSynchronized`.
pub fn parse_ntp_synchronized(text: &str) -> Result<bool, SourceError> {
    match text.trim() {
        "yes" => Ok(true),
        "no" => Ok(false),
        other => Err(SourceError(format!("unexpected NTPSynchronized value {other:?}"))),
    }
}

pub struct TimedatectlSource;

impl ClockSource for TimedatectlSource {
    fn is_synchronized(&self) -> Result<bool, SourceError> {
        let out = Command::new("timedatectl")
            .args(["show", "-p", "NTPSynchronized", "--value"])
            .output()
            .map_err(|e| SourceError(format!("spawn timedatectl: {e}")))?;
        if !out.status.success() {
            return Err(SourceError(format!(
                "timedatectl exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        parse_ntp_synchronized(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yes_and_no() {
        assert_eq!(parse_ntp_synchronized("yes\n"), Ok(true));
        assert_eq!(parse_ntp_synchronized("no\n"), Ok(false));
        assert_eq!(parse_ntp_synchronized(" yes "), Ok(true));
    }

    #[test]
    fn rejects_anything_else() {
        assert!(parse_ntp_synchronized("").is_err());
        assert!(parse_ntp_synchronized("maybe").is_err());
        assert!(parse_ntp_synchronized("Failed to connect to bus").is_err());
    }
}
