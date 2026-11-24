//! `DebugAccessCtl` over `systemctl`. Every argv is fixed or built from the
//! `Surface` enum and the four configured unit names, never from a client
//! string, and no shell is involved. Mirrors `usb_role_cli.rs`'s shape; the
//! one difference is a timeout on every call (plan section 8's device
//! checklist: a toggle must never hang the mutation mutex).
//!
//! Unit roles (brief and spike report):
//! - ADB enable: `systemctl start <adb_unit>` (`android-tools-adbd.service`).
//! - ADB disable: `systemctl start <adb_disabled_unit>`
//!   (`thing-revived-usb-bind.service`); its own `Conflicts=` stops adbd.
//! - UART enable: `systemctl start <uart_unit>`
//!   (`thing-revived-uart-root@ttyAML0.service`); its `Conflicts=` stops
//!   the stock getty.
//! - UART disable: `systemctl stop <uart_unit>` then
//!   `systemctl start <uart_disabled_unit>` (`serial-getty@ttyAML0.service`),
//!   because systemd does not restart a conflicting unit just because the
//!   one that stopped it stopped in turn.
//! - Status: `systemctl is-active <adb_unit>` / `<uart_unit>`; the enabling
//!   unit's activation is what "this surface is on" means.
//! - Build default (T-D3.1): adb's build-time default is sourced
//!   elsewhere, from `UsbRoleCtl::adb_forced` (`/etc/usb-debugging-enabled`,
//!   the same file the build side already wrote); `build_default(Adb)`
//!   here is never called by the service and answers `false` only to
//!   satisfy the trait. uart's build default has no such file, so it is
//!   `systemctl is-enabled <uart_unit>`: `enabled` or `enabled-runtime`
//!   means the unit starts at boot, anything else (`disabled`,
//!   `static`, `masked`, a non-zero exit, a spawn failure) is "not the
//!   default", fail-safe toward never claiming a wrong build default.

use crate::domain::debug_access::{parse_active, Surface};
use crate::ports::DebugAccessCtl;
use crate::utils::config::DebugAccessConfig;
use crate::utils::types::AppError;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct DebugAccessCli {
    systemctl: String,
    adb_unit: String,
    adb_disabled_unit: String,
    uart_unit: String,
    uart_disabled_unit: String,
    timeout: Duration,
}

impl DebugAccessCli {
    pub fn new(cfg: &DebugAccessConfig) -> DebugAccessCli {
        DebugAccessCli {
            systemctl: cfg.systemctl_command.clone(),
            adb_unit: cfg.adb_unit.clone(),
            adb_disabled_unit: cfg.adb_disabled_unit.clone(),
            uart_unit: cfg.uart_unit.clone(),
            uart_disabled_unit: cfg.uart_disabled_unit.clone(),
            timeout: Duration::from_secs(cfg.command_timeout_secs),
        }
    }

    fn enable_unit(&self, surface: Surface) -> &str {
        match surface {
            Surface::Adb => &self.adb_unit,
            Surface::Uart => &self.uart_unit,
        }
    }

    fn disable_unit(&self, surface: Surface) -> &str {
        match surface {
            Surface::Adb => &self.adb_disabled_unit,
            Surface::Uart => &self.uart_disabled_unit,
        }
    }

    /// Runs `<systemctl> <args>` with no shell, killing the child and
    /// failing with `toggle_failed`-mapped text if it outlives the
    /// configured timeout. Stdout is returned trimmed; a non-zero exit
    /// carries stderr in the error message.
    fn run(&self, args: &[&str]) -> Result<String, AppError> {
        let label = format!("{} {}", self.systemctl, args.join(" "));
        let mut child = Command::new(&self.systemctl)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::failed(format!("spawn {label}: {e}")))?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout = String::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = std::io::Read::read_to_string(&mut out, &mut stdout);
                    }
                    let mut stderr = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut err, &mut stderr);
                    }
                    if !status.success() {
                        return Err(AppError::failed(format!("{label} failed: {}", stderr.trim())));
                    }
                    return Ok(stdout.trim().to_string());
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
}

impl DebugAccessCtl for DebugAccessCli {
    fn status(&self, surface: Surface) -> Result<bool, AppError> {
        // `is-active` exits non-zero for every state but `active`; that is
        // not a failure here, it is the fail-safe "disabled" answer, so the
        // exit code is ignored and only the printed word is read. A run
        // that could not even start the tool still errors.
        let unit = self.enable_unit(surface).to_string();
        let out = Command::new(&self.systemctl)
            .args(["is-active", &unit])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| AppError::failed(format!("spawn {} is-active {unit}: {e}", self.systemctl)))?;
        Ok(parse_active(&String::from_utf8_lossy(&out.stdout)))
    }

    fn enable(&self, surface: Surface) -> Result<(), AppError> {
        let unit = self.enable_unit(surface).to_string();
        self.run(&["start", &unit]).map(|_| ())
    }

    fn disable(&self, surface: Surface) -> Result<(), AppError> {
        let disabled_unit = self.disable_unit(surface).to_string();
        if surface == Surface::Uart {
            // Unlike the adb pair, stopping the UART root unit does not
            // auto-restart the getty it conflicted with: a disable needs
            // the explicit second call (spike item (d), plan section 4.4).
            let stop_unit = self.enable_unit(surface).to_string();
            self.run(&["stop", &stop_unit])?;
        }
        self.run(&["start", &disabled_unit]).map(|_| ())
    }

    fn build_default(&self, surface: Surface) -> Result<bool, AppError> {
        let Surface::Uart = surface else {
            // adb's build default comes from `UsbRoleCtl::adb_forced`
            // (the `/etc/usb-debugging-enabled` marker), not from here;
            // the service never calls this arm.
            return Ok(false);
        };
        let unit = self.enable_unit(Surface::Uart).to_string();
        let out = Command::new(&self.systemctl)
            .args(["is-enabled", &unit])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| AppError::failed(format!("spawn {} is-enabled {unit}: {e}", self.systemctl)))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let word = text.trim();
        Ok(word == "enabled" || word == "enabled-runtime")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_util::lock;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A stand-in `systemctl`: a shell script that records its arguments in
    /// `args` next to it and then runs `body`.
    fn rig(name: &str, body: &str) -> (PathBuf, DebugAccessCli, std::sync::MutexGuard<'static, ()>) {
        let guard = lock(&crate::sync_util::SPAWN_GUARD);
        let dir = std::env::temp_dir().join(format!("controller-debugaccess-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("systemctl");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/args\"\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tool = script.to_string_lossy().into_owned();
        let cli = DebugAccessCli::new(&DebugAccessConfig {
            systemctl_command: tool,
            adb_unit: "android-tools-adbd.service".into(),
            adb_disabled_unit: "thing-revived-usb-bind.service".into(),
            uart_unit: "thing-revived-uart-root@ttyAML0.service".into(),
            uart_disabled_unit: "serial-getty@ttyAML0.service".into(),
            command_timeout_secs: 5,
        });
        (dir, cli, guard)
    }

    fn args(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("args")).unwrap().lines().map(str::to_string).collect()
    }

    #[test]
    fn status_runs_is_active_on_the_enabling_unit_and_parses_fail_safe() {
        // Each block drops its `SPAWN_GUARD` guard before the next `rig`
        // call: shadowing `_g` does not drop the earlier guard until the
        // whole function returns, which would self-deadlock the mutex.
        {
            let (dir, cli, _g) = rig("status-adb", "printf 'active\\n'");
            assert!(cli.status(Surface::Adb).unwrap());
            assert_eq!(args(&dir), vec!["is-active android-tools-adbd.service"]);
            let _ = std::fs::remove_dir_all(dir);
        }
        {
            let (dir, cli, _g) = rig("status-uart-off", "printf 'inactive\\n'; exit 3");
            assert!(!cli.status(Surface::Uart).unwrap());
            assert_eq!(args(&dir), vec!["is-active thing-revived-uart-root@ttyAML0.service"]);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn status_garbage_output_reads_as_disabled_not_an_error() {
        let (dir, cli, _g) = rig("status-garbage", "printf 'activating\\n'; exit 3");
        assert!(!cli.status(Surface::Adb).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enable_adb_starts_the_adb_unit_with_fixed_argv() {
        let (dir, cli, _g) = rig("enable-adb", "");
        cli.enable(Surface::Adb).unwrap();
        assert_eq!(args(&dir), vec!["start android-tools-adbd.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enable_uart_starts_the_uart_unit_with_fixed_argv() {
        let (dir, cli, _g) = rig("enable-uart", "");
        cli.enable(Surface::Uart).unwrap();
        assert_eq!(args(&dir), vec!["start thing-revived-uart-root@ttyAML0.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disable_adb_starts_the_usb_bind_unit_only() {
        let (dir, cli, _g) = rig("disable-adb", "");
        cli.disable(Surface::Adb).unwrap();
        assert_eq!(args(&dir), vec!["start thing-revived-usb-bind.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disable_uart_stops_the_root_unit_then_starts_the_stock_getty() {
        let (dir, cli, _g) = rig("disable-uart", "");
        cli.disable(Surface::Uart).unwrap();
        assert_eq!(
            args(&dir),
            vec!["stop thing-revived-uart-root@ttyAML0.service", "start serial-getty@ttyAML0.service"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disable_uart_stops_trying_if_the_stop_itself_fails() {
        let (dir, cli, _g) = rig("disable-uart-fail", "case \"$1\" in stop) echo boom >&2; exit 1;; esac");
        let err = cli.disable(Surface::Uart).unwrap_err();
        assert!(err.message.contains("boom"), "{err}");
        assert_eq!(args(&dir), vec!["stop thing-revived-uart-root@ttyAML0.service"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failing_tool_is_a_failed_error_carrying_stderr() {
        let (dir, cli, _g) = rig("fail", "echo 'unit not found' >&2; exit 1");
        let err = cli.enable(Surface::Adb).unwrap_err();
        assert!(err.message.contains("unit not found"), "{err}");
        let err = cli.disable(Surface::Adb).unwrap_err();
        assert!(err.message.contains("unit not found"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hanging_tool_is_killed_and_reported_as_timed_out() {
        let (dir, cli, _g) = rig("hang", "sleep 30");
        let mut cli = cli;
        cli.timeout = Duration::from_millis(100);
        let err = cli.enable(Surface::Adb).unwrap_err();
        assert!(err.message.contains("timed out"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let cli = DebugAccessCli::new(&DebugAccessConfig { systemctl_command: "/nonexistent/systemctl".into(), ..DebugAccessConfig::default() });
        assert!(cli.status(Surface::Adb).is_err());
        assert!(cli.enable(Surface::Uart).is_err());
        assert!(cli.disable(Surface::Uart).is_err());
        assert!(cli.build_default(Surface::Uart).is_err());
    }

    #[test]
    fn build_default_adb_never_runs_the_tool_and_answers_false() {
        // adb's build default is sourced from `UsbRoleCtl::adb_forced`
        // elsewhere; this arm exists only to satisfy the trait.
        let (dir, cli, _g) = rig("build-default-adb", "echo should-not-run; exit 1");
        assert!(!cli.build_default(Surface::Adb).unwrap());
        assert!(!dir.join("args").exists(), "the tool must never be spawned for adb");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_default_uart_runs_is_enabled_on_the_uart_unit() {
        for (word, expected) in [("enabled", true), ("enabled-runtime", true), ("disabled", false), ("static", false), ("masked", false)] {
            let (dir, cli, _g) = rig("build-default-uart", &format!("printf '{word}\\n'"));
            assert_eq!(cli.build_default(Surface::Uart).unwrap(), expected, "{word}");
            assert_eq!(args(&dir), vec!["is-enabled thing-revived-uart-root@ttyAML0.service"]);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn build_default_uart_a_nonzero_exit_reads_as_not_default_not_an_error() {
        // `is-enabled` exits non-zero for `disabled`/`masked` by design;
        // that is the fail-safe answer, not a failure, matching `status`.
        let (dir, cli, _g) = rig("build-default-uart-nonzero", "printf 'disabled\\n'; exit 1");
        assert!(!cli.build_default(Surface::Uart).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_default_uart_garbage_output_reads_as_not_default() {
        let (dir, cli, _g) = rig("build-default-uart-garbage", "printf 'enabled-extra-junk\\n'");
        assert!(!cli.build_default(Surface::Uart).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
