//! `UsbRoleCtl` over `superbird-usb-role` and `systemctl`. Every argv is
//! fixed or built from the `UsbRole` enum, never from a client string, and no
//! shell is involved. Only `status` and `persist` are used: `to-host` and
//! `to-device` switch live and drop the gadget link. Program names and the
//! adb flag path come from `[usb_role]` in controller.toml.

use crate::app::error::AppError;
use crate::domain::usb_role::{parse_status, UsbRole, UsbRoleStatus};
use crate::ports::UsbRoleCtl;
use crate::utils::config::UsbRoleConfig;
use std::path::PathBuf;
use std::process::Command;

pub struct UsbRoleCli {
    command: String,
    systemctl: String,
    adb_flag: PathBuf,
}

impl UsbRoleCli {
    pub fn new(cfg: &UsbRoleConfig) -> UsbRoleCli {
        UsbRoleCli { command: cfg.command.clone(), systemctl: cfg.systemctl_command.clone(), adb_flag: PathBuf::from(&cfg.adb_flag_path) }
    }

    fn run(program: &str, args: &[&str]) -> Result<String, AppError> {
        let label = format!("{program} {}", args.join(" "));
        let out = Command::new(program).args(args).output().map_err(|e| AppError::failed(format!("spawn {label}: {e}")))?;
        if !out.status.success() {
            return Err(AppError::failed(format!("{label} failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl UsbRoleCtl for UsbRoleCli {
    fn status(&self) -> Result<UsbRoleStatus, AppError> {
        Ok(parse_status(&UsbRoleCli::run(&self.command, &["status"])?))
    }

    fn persist(&self, role: UsbRole) -> Result<(), AppError> {
        UsbRoleCli::run(&self.command, &["persist", role.as_str()]).map(|_| ())
    }

    fn reboot(&self) -> Result<(), AppError> {
        UsbRoleCli::run(&self.systemctl, &["reboot"]).map(|_| ())
    }

    fn adb_forced(&self) -> bool {
        self.adb_flag.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_util::lock;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in tool: a shell script that records its arguments in `args`
    /// next to it and then runs `body`.
    fn rig(name: &str, body: &str) -> (PathBuf, UsbRoleCli, std::sync::MutexGuard<'static, ()>) {
        let guard = lock(&crate::sync_util::SPAWN_GUARD);
        let dir = std::env::temp_dir().join(format!("controller-usbrole-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tool");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/args\"\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tool = script.to_string_lossy().into_owned();
        let cli = UsbRoleCli::new(&UsbRoleConfig {
            command: tool.clone(),
            systemctl_command: tool,
            adb_flag_path: dir.join("usb-debugging-enabled").to_string_lossy().into_owned(),
        });
        (dir, cli, guard)
    }

    fn args(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("args")).unwrap().lines().map(str::to_string).collect()
    }

    #[test]
    fn status_runs_the_fixed_argv_and_parses_stdout() {
        let (dir, cli, _g) = rig("status", "printf 'role: device\\nrevert: none\\nboot role: host\\n'");
        let s = cli.status().unwrap();
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Device), boot_role: Some(UsbRole::Host) });
        assert_eq!(args(&dir), vec!["status"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persist_passes_the_role_as_a_separate_argument_and_never_switches_live() {
        let (dir, cli, _g) = rig("persist", "");
        cli.persist(UsbRole::Host).unwrap();
        cli.persist(UsbRole::Device).unwrap();
        assert_eq!(args(&dir), vec!["persist host", "persist device"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reboot_runs_systemctl_reboot() {
        let (dir, cli, _g) = rig("reboot", "");
        cli.reboot().unwrap();
        assert_eq!(args(&dir), vec!["reboot"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failing_tool_is_a_failed_error_carrying_stderr() {
        let (dir, cli, _g) = rig("fail", "echo 'read-only file system' >&2; exit 1");
        assert!(cli.status().unwrap_err().message.contains("read-only file system"));
        assert!(cli.persist(UsbRole::Host).is_err());
        assert!(cli.reboot().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let cli = UsbRoleCli::new(&UsbRoleConfig { command: "/nonexistent/usb-role".into(), ..UsbRoleConfig::default() });
        assert!(cli.status().is_err());
        assert!(cli.persist(UsbRole::Device).is_err());
    }

    #[test]
    fn adb_forced_follows_the_flag_file() {
        let (dir, cli, _g) = rig("adb", "");
        assert!(!cli.adb_forced());
        std::fs::write(dir.join("usb-debugging-enabled"), "").unwrap();
        assert!(cli.adb_forced());
        let _ = std::fs::remove_dir_all(dir);
    }
}
