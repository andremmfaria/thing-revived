//! Timezone get/set/list via `timedatectl`. See
//! docs/research/settings-menu-feasibility.md section 7. Deliberately does NOT
//! add a "set system time" control: that section reasons through why it
//! would be redundant with (and worse than) the terminal agent's existing SSH
//! clock-sync offset. The program name comes from `[timezone] command`.

use crate::app::error::AppError;
use crate::ports::TimezoneCtl;
use crate::utils::config::TimezoneConfig;
use log::error;
use std::process::Command;

pub struct Timedatectl {
    command: String,
}

impl Timedatectl {
    pub fn new(cfg: &TimezoneConfig) -> Timedatectl {
        Timedatectl { command: cfg.command.clone() }
    }
}

impl TimezoneCtl for Timedatectl {
    /// Enumerates every valid IANA zone name `set-timezone` will accept.
    /// Called once at startup and cached by the service: this list changes
    /// only with a `tzdata` package update, never at runtime.
    fn list(&self) -> Vec<String> {
        match Command::new(&self.command).arg("list-timezones").output() {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            Ok(out) => {
                error!("{} list-timezones failed: {}", self.command, String::from_utf8_lossy(&out.stderr));
                Vec::new()
            }
            Err(e) => {
                error!("failed to spawn {} list-timezones: {e}", self.command);
                Vec::new()
            }
        }
    }

    fn get(&self) -> Result<String, AppError> {
        let out = Command::new(&self.command)
            .args(["show", "--property=Timezone", "--value"])
            .output()
            .map_err(|e| AppError::failed(format!("spawn timedatectl show: {e}")))?;
        if !out.status.success() {
            return Err(AppError::failed(format!("timedatectl show failed: {}", String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn set(&self, tz: &str) -> Result<(), AppError> {
        let out = Command::new(&self.command)
            .args(["set-timezone", tz])
            .output()
            .map_err(|e| AppError::failed(format!("spawn timedatectl set-timezone: {e}")))?;
        if !out.status.success() {
            return Err(AppError::failed(format!(
                "timedatectl set-timezone failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_util::lock;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `timedatectl`: a shell script that records its arguments.
    fn fake_command(name: &str, body: &str) -> (std::path::PathBuf, Timedatectl, std::sync::MutexGuard<'static, ()>) {
        let guard = lock(&crate::sync_util::SPAWN_GUARD);
        let dir = std::env::temp_dir().join(format!("controller-tz-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("timedatectl");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tz = Timedatectl::new(&TimezoneConfig { command: script.to_string_lossy().into_owned() });
        (dir, tz, guard)
    }

    #[test]
    fn list_splits_lines_and_drops_blanks() {
        let (dir, tz, _guard) = fake_command("list", r#"printf 'Europe/Lisbon\n\n  UTC  \n'"#);
        assert_eq!(tz.list(), vec!["Europe/Lisbon".to_string(), "UTC".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn get_trims_and_set_passes_the_zone_as_one_argument() {
        let (dir, tz, _guard) = fake_command("getset", r#"if [ "$1" = show ]; then echo Europe/Lisbon; else echo "$@" > "$(dirname "$0")/args"; fi"#);
        assert_eq!(tz.get().unwrap(), "Europe/Lisbon");
        tz.set("America/New_York").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("args")).unwrap().trim(), "set-timezone America/New_York");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failing_command_is_a_failed_error_and_an_empty_list() {
        let (dir, tz, _guard) = fake_command("fail", "echo nope >&2; exit 1");
        assert!(tz.list().is_empty());
        assert!(tz.get().unwrap_err().message.contains("nope"));
        assert!(tz.set("UTC").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let tz = Timedatectl::new(&TimezoneConfig { command: "/nonexistent/timedatectl".into() });
        assert!(tz.list().is_empty());
        assert!(tz.get().is_err());
    }
}
