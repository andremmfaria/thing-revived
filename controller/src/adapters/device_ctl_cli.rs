//! `DeviceCtl` (and the boot apply's `BootCtl`) over `systemctl` and
//! `superbird-usb-role`. Every argv is fixed or built from enums and the
//! configured unit names, never from a client string, and no shell is
//! involved. Every call has a timeout (`[device_settings]
//! command_timeout_secs`) so a stuck tool cannot hold the mutation mutex.
//!
//! Per setting (plan sections 5.1 and 5.2, spike report):
//! - ssh on: remove the `ssh-off` flag, then `start sshd.socket` (without
//!   the removal the start is a silent no-op, the gate condition fails).
//! - ssh off: create the flag, `stop sshd.socket`, then `stop 'sshd@*.service'`
//!   (the glob is one argv element, systemctl expands it; a glob that
//!   matches no loaded instance is not an error).
//! - adb on: `stop usb-bind`, then `start adbd`. The zz drop-in relinks
//!   `ffs.adb` in `ExecStartPre` and `Conflicts=` is the safety net.
//! - adb off: `start usb-bind`; its `Conflicts=` stops adbd and rebinds the
//!   gadget NCM-only.
//! - uart on: `start uart-root`; its `Conflicts=` stops the stock getty.
//! - uart off: `stop uart-root`, then `start serial-getty` (systemd does not
//!   revive the Conflicts= loser by itself).
//! - to-host: `superbird-usb-role to-host --no-revert` (never the plain form,
//!   which schedules a 300 s revert).
//! - to-device: `to-device`, then `start usb-bind` when neither usb-bind nor
//!   adbd is active, so the NCM-only gadget is bound. When the UDC stayed
//!   bound the link returns by itself (spike P0.3) and nothing is started.

use crate::adapters::device_settings_json::{create_flag, remove_flag};
use crate::boot_apply::{BootCtl, Unit};
use crate::domain::device_settings::{parse_active, parse_status, Setting, UsbRole, UsbRoleStatus};
use crate::ports::DeviceCtl;
use crate::utils::config::DeviceSettingsConfig;
use crate::utils::types::AppError;
use log::warn;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct DeviceCtlCli {
    systemctl: String,
    usb_role: String,
    adb_unit: String,
    usb_bind_unit: String,
    uart_unit: String,
    getty_unit: String,
    ssh_socket: String,
    ssh_glob: String,
    ssh_off_flag: PathBuf,
    timeout: Duration,
}

/// What one finished tool call printed. Exit status and output are kept
/// separately because `is-active` exits non-zero for every state but active.
struct Done {
    success: bool,
    stdout: String,
    stderr: String,
}

impl DeviceCtlCli {
    pub fn new(cfg: &DeviceSettingsConfig) -> DeviceCtlCli {
        DeviceCtlCli {
            systemctl: cfg.systemctl_command.clone(),
            usb_role: cfg.usb_role_command.clone(),
            adb_unit: cfg.adb_unit.clone(),
            usb_bind_unit: cfg.adb_disabled_unit.clone(),
            uart_unit: cfg.uart_unit.clone(),
            getty_unit: cfg.uart_disabled_unit.clone(),
            ssh_socket: cfg.ssh_socket_unit.clone(),
            ssh_glob: cfg.ssh_instance_glob.clone(),
            ssh_off_flag: PathBuf::from(&cfg.ssh_off_flag_path),
            timeout: Duration::from_secs(cfg.command_timeout_secs),
        }
    }

    /// Runs `program args` with no shell, killing it when it outlives the
    /// timeout. A tool that cannot be found is `unavailable`.
    fn exec(&self, program: &str, args: &[&str]) -> Result<Done, AppError> {
        let label = format!("{program} {}", args.join(" "));
        let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                AppError::unavailable(format!("{program} not found"))
            } else {
                AppError::failed(format!("spawn {label}: {e}"))
            }
        })?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout = String::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_string(&mut stdout);
                    }
                    let mut stderr = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_string(&mut stderr);
                    }
                    return Ok(Done { success: status.success(), stdout, stderr });
                }
                Ok(None) if start.elapsed() >= self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::failed(format!("{label} timed out")));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(AppError::failed(format!("wait {label}: {e}"))),
            }
        }
    }

    /// Like `exec` but a non-zero exit is an error carrying stderr.
    fn run(&self, program: &str, args: &[&str]) -> Result<String, AppError> {
        let done = self.exec(program, args)?;
        if !done.success {
            return Err(AppError::failed(format!("{program} {} failed: {}", args.join(" "), done.stderr.trim())));
        }
        Ok(done.stdout)
    }

    fn systemctl(&self, args: &[&str]) -> Result<(), AppError> {
        self.run(&self.systemctl, args).map(|_| ())
    }

    fn role(&self, args: &[&str]) -> Result<String, AppError> {
        self.run(&self.usb_role, args)
    }

    /// `is-active`: only the exact word `active` is on, the exit code is ignored.
    fn active(&self, unit: &str) -> Result<bool, AppError> {
        Ok(parse_active(&self.exec(&self.systemctl, &["is-active", unit])?.stdout))
    }

    fn unit_name(&self, unit: Unit) -> &str {
        match unit {
            Unit::Adbd => &self.adb_unit,
            Unit::UsbBind => &self.usb_bind_unit,
            Unit::UartRoot => &self.uart_unit,
            Unit::Getty => &self.getty_unit,
        }
    }
}

impl DeviceCtl for DeviceCtlCli {
    fn usb_status(&self) -> Result<UsbRoleStatus, AppError> {
        Ok(parse_status(&self.role(&["status"])?))
    }

    fn is_on(&self, setting: Setting) -> Result<bool, AppError> {
        match setting {
            Setting::Ssh => self.active(&self.ssh_socket),
            Setting::Adb => self.active(&self.adb_unit),
            Setting::Uart => self.active(&self.uart_unit),
            Setting::Mode => Err(AppError::invalid("mode is not a unit, use usb_status")),
        }
    }

    fn set_live(&self, setting: Setting, on: bool) -> Result<(), AppError> {
        match (setting, on) {
            (Setting::Ssh, true) => {
                remove_flag(&self.ssh_off_flag)?;
                self.systemctl(&["start", &self.ssh_socket])
            }
            (Setting::Ssh, false) => {
                create_flag(&self.ssh_off_flag)?;
                self.systemctl(&["stop", &self.ssh_socket])?;
                // Existing connections are separate Accept=yes instances. A
                // glob that matches nothing makes systemctl complain, which
                // only means there was no session to end.
                if let Err(e) = self.systemctl(&["stop", &self.ssh_glob]) {
                    warn!("device-ctl: stopping ssh instances: {e}");
                }
                Ok(())
            }
            (Setting::Adb, true) => {
                self.systemctl(&["stop", &self.usb_bind_unit])?;
                self.systemctl(&["start", &self.adb_unit])
            }
            (Setting::Adb, false) => self.systemctl(&["start", &self.usb_bind_unit]),
            (Setting::Uart, true) => self.systemctl(&["start", &self.uart_unit]),
            (Setting::Uart, false) => {
                self.systemctl(&["stop", &self.uart_unit])?;
                self.systemctl(&["start", &self.getty_unit])
            }
            (Setting::Mode, _) => Err(AppError::invalid("mode is switched with to_host or to_device")),
        }
    }

    fn to_host(&self) -> Result<(), AppError> {
        self.role(&["to-host", "--no-revert"]).map(|_| ())
    }

    fn to_device(&self) -> Result<(), AppError> {
        self.role(&["to-device"])?;
        if !self.active(&self.usb_bind_unit)? && !self.active(&self.adb_unit)? {
            self.systemctl(&["start", &self.usb_bind_unit])?;
        }
        Ok(())
    }

    fn persist_boot_role(&self, role: UsbRole) -> Result<(), AppError> {
        self.role(&["persist", role.as_str()]).map(|_| ())
    }

    fn reboot(&self) -> Result<(), AppError> {
        self.systemctl(&["reboot"])
    }
}

impl BootCtl for DeviceCtlCli {
    fn is_active(&self, unit: Unit) -> bool {
        self.active(self.unit_name(unit)).unwrap_or(false)
    }

    fn start(&self, unit: Unit) -> Result<(), AppError> {
        self.systemctl(&["start", self.unit_name(unit)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_util::lock;
    use std::os::unix::fs::PermissionsExt;

    /// Two stand-in tools (`systemctl`, `superbird-usb-role`) that append
    /// `<tool> <args>` to a shared `args` file and then run `body` (the
    /// shell sees the tool's own arguments in `$@`). The ssh flag state at
    /// the moment of every call is appended as `flag=...` when `probe_flag`.
    fn rig(name: &str, body: &str, probe_flag: bool) -> (PathBuf, DeviceCtlCli, std::sync::MutexGuard<'static, ()>) {
        let guard = lock(&crate::sync_util::SPAWN_GUARD);
        let dir = std::env::temp_dir().join(format!("controller-devctl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let flag = dir.join("run/ssh-off");
        let probe = if probe_flag { format!("echo \"flag=$(test -e '{}' && echo present || echo absent)\" >> \"$D/args\"\n", flag.display()) } else { String::new() };
        for tool in ["systemctl", "superbird-usb-role"] {
            let script = dir.join(tool);
            std::fs::write(&script, format!("#!/bin/sh\nD=\"$(dirname \"$0\")\"\necho \"{tool} $*\" >> \"$D/args\"\n{probe}{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = DeviceSettingsConfig {
            systemctl_command: dir.join("systemctl").to_string_lossy().into_owned(),
            usb_role_command: dir.join("superbird-usb-role").to_string_lossy().into_owned(),
            ssh_off_flag_path: flag.to_string_lossy().into_owned(),
            command_timeout_secs: 5,
            ..DeviceSettingsConfig::default()
        };
        (dir, DeviceCtlCli::new(&cfg), guard)
    }

    fn args(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("args")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    fn flag(dir: &std::path::Path) -> bool {
        dir.join("run/ssh-off").exists()
    }

    #[test]
    fn ssh_on_removes_the_flag_before_starting_the_socket() {
        let (dir, cli, _g) = rig("ssh-on", "", true);
        std::fs::write(dir.join("run/ssh-off"), "").unwrap();
        cli.set_live(Setting::Ssh, true).unwrap();
        assert_eq!(args(&dir), vec!["systemctl start sshd.socket", "flag=absent"]);
        assert!(!flag(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_off_creates_the_flag_then_stops_socket_then_instances() {
        let (dir, cli, _g) = rig("ssh-off", "", true);
        cli.set_live(Setting::Ssh, false).unwrap();
        assert_eq!(
            args(&dir),
            vec!["systemctl stop sshd.socket", "flag=present", "systemctl stop sshd@*.service", "flag=present"]
        );
        assert!(flag(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_off_tolerates_an_instance_glob_that_matches_nothing() {
        let (dir, cli, _g) = rig("ssh-glob", "case \"$2\" in 'sshd@*.service') echo 'not loaded' >&2; exit 5;; esac", false);
        cli.set_live(Setting::Ssh, false).unwrap();
        assert_eq!(args(&dir), vec!["systemctl stop sshd.socket", "systemctl stop sshd@*.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_off_fails_when_the_socket_stop_fails_but_the_flag_is_already_set() {
        let (dir, cli, _g) = rig("ssh-fail", "exit 1", false);
        assert!(cli.set_live(Setting::Ssh, false).is_err());
        assert!(flag(&dir), "restrict first: the gate is closed even when the stop failed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adb_on_stops_usb_bind_then_starts_adbd() {
        let (dir, cli, _g) = rig("adb-on", "", false);
        cli.set_live(Setting::Adb, true).unwrap();
        assert_eq!(args(&dir), vec!["systemctl stop thing-revived-usb-bind.service", "systemctl start android-tools-adbd.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adb_off_starts_usb_bind_only() {
        let (dir, cli, _g) = rig("adb-off", "", false);
        cli.set_live(Setting::Adb, false).unwrap();
        assert_eq!(args(&dir), vec!["systemctl start thing-revived-usb-bind.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adb_on_does_not_start_adbd_when_the_usb_bind_stop_fails() {
        let (dir, cli, _g) = rig("adb-on-fail", "exit 1", false);
        assert!(cli.set_live(Setting::Adb, true).is_err());
        assert_eq!(args(&dir), vec!["systemctl stop thing-revived-usb-bind.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn uart_on_starts_the_root_console_and_off_stops_it_then_starts_the_stock_getty() {
        let (dir, cli, _g) = rig("uart", "", false);
        cli.set_live(Setting::Uart, true).unwrap();
        cli.set_live(Setting::Uart, false).unwrap();
        assert_eq!(
            args(&dir),
            vec![
                "systemctl start thing-revived-uart-root@ttyAML0.service",
                "systemctl stop thing-revived-uart-root@ttyAML0.service",
                "systemctl start serial-getty@ttyAML0.service",
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn uart_off_does_not_start_the_getty_when_the_stop_fails() {
        let (dir, cli, _g) = rig("uart-fail", "exit 1", false);
        assert!(cli.set_live(Setting::Uart, false).is_err());
        assert_eq!(args(&dir), vec!["systemctl stop thing-revived-uart-root@ttyAML0.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mode_is_not_a_live_unit_setting() {
        let (dir, cli, _g) = rig("mode", "", false);
        assert!(cli.set_live(Setting::Mode, true).is_err());
        assert!(cli.is_on(Setting::Mode).is_err());
        assert!(args(&dir).is_empty(), "nothing is run for mode");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn to_host_never_uses_the_auto_reverting_form() {
        let (dir, cli, _g) = rig("to-host", "", false);
        cli.to_host().unwrap();
        assert_eq!(args(&dir), vec!["superbird-usb-role to-host --no-revert"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn to_device_rebinds_the_gadget_when_nothing_holds_it() {
        let (dir, cli, _g) = rig("to-device-cold", "case \"$1\" in is-active) echo inactive; exit 3;; esac", false);
        cli.to_device().unwrap();
        assert_eq!(
            args(&dir),
            vec![
                "superbird-usb-role to-device",
                "systemctl is-active thing-revived-usb-bind.service",
                "systemctl is-active android-tools-adbd.service",
                "systemctl start thing-revived-usb-bind.service",
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn to_device_leaves_a_gadget_that_stayed_bound_alone() {
        let (dir, cli, _g) = rig("to-device-warm", "case \"$1\" in is-active) echo active;; esac", false);
        cli.to_device().unwrap();
        assert_eq!(args(&dir), vec!["superbird-usb-role to-device", "systemctl is-active thing-revived-usb-bind.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_persist_reboot_and_is_active_use_the_fixed_argv() {
        let (dir, cli, _g) = rig("misc", "case \"$1\" in status) printf 'role: host\\nrevert: none\\nboot role: device\\n';; is-active) echo active;; esac", false);
        let s = cli.usb_status().unwrap();
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: Some(UsbRole::Device) });
        cli.persist_boot_role(UsbRole::Host).unwrap();
        cli.persist_boot_role(UsbRole::Device).unwrap();
        cli.reboot().unwrap();
        assert!(cli.is_on(Setting::Ssh).unwrap() && cli.is_on(Setting::Adb).unwrap() && cli.is_on(Setting::Uart).unwrap());
        assert_eq!(
            args(&dir),
            vec![
                "superbird-usb-role status",
                "superbird-usb-role persist host",
                "superbird-usb-role persist device",
                "systemctl reboot",
                "systemctl is-active sshd.socket",
                "systemctl is-active android-tools-adbd.service",
                "systemctl is-active thing-revived-uart-root@ttyAML0.service",
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn is_active_reads_only_the_exact_word_and_ignores_the_exit_code() {
        for (out, want) in [("active", true), ("inactive", false), ("activating", false), ("failed", false), ("", false), ("active-ish", false)] {
            let (dir, cli, _g) = rig("active", &format!("echo '{out}'; exit 3"), false);
            assert_eq!(cli.is_on(Setting::Adb).unwrap(), want, "{out:?}");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_failing_tool_is_an_error_carrying_stderr() {
        let (dir, cli, _g) = rig("fail", "echo 'Access denied' >&2; exit 1", false);
        assert!(cli.set_live(Setting::Uart, true).unwrap_err().message.contains("Access denied"));
        assert!(cli.to_host().is_err());
        assert!(cli.persist_boot_role(UsbRole::Host).is_err());
        assert!(cli.reboot().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_program_is_unavailable_not_a_panic() {
        let cfg = DeviceSettingsConfig {
            systemctl_command: "/nonexistent/systemctl".into(),
            usb_role_command: "/nonexistent/usb-role".into(),
            ..DeviceSettingsConfig::default()
        };
        let cli = DeviceCtlCli::new(&cfg);
        assert_eq!(cli.usb_status().unwrap_err().kind, crate::utils::types::ErrorKind::Unavailable);
        assert!(cli.set_live(Setting::Adb, false).is_err());
        assert!(!cli.is_active(Unit::Adbd), "an unreadable state is off (fail safe)");
    }

    #[test]
    fn a_tool_that_outlives_the_timeout_is_killed() {
        let (dir, mut cli, _g) = rig("timeout", "exec sleep 5", false);
        cli.timeout = Duration::from_millis(300);
        let start = Instant::now();
        let err = cli.set_live(Setting::Adb, false).unwrap_err();
        assert!(err.message.contains("timed out"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn boot_ctl_maps_each_unit_to_its_configured_name() {
        let (dir, cli, _g) = rig("bootctl", "case \"$1\" in is-active) echo active;; esac", false);
        for unit in [Unit::Adbd, Unit::UsbBind, Unit::UartRoot, Unit::Getty] {
            assert!(cli.is_active(unit));
            cli.start(unit).unwrap();
        }
        assert_eq!(
            args(&dir),
            vec![
                "systemctl is-active android-tools-adbd.service",
                "systemctl start android-tools-adbd.service",
                "systemctl is-active thing-revived-usb-bind.service",
                "systemctl start thing-revived-usb-bind.service",
                "systemctl is-active thing-revived-uart-root@ttyAML0.service",
                "systemctl start thing-revived-uart-root@ttyAML0.service",
                "systemctl is-active serial-getty@ttyAML0.service",
                "systemctl start serial-getty@ttyAML0.service",
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
