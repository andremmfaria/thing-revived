//! Boot apply (`controller-agent apply-device-settings`): early, late and
//! failsafe rules with recording fakes, the real adapters over a scratch
//! directory, the layering rule that the early path never reaches a unit
//! control, and the pin between the compiled-in constants and the shipped
//! `config/controller.toml`.

use controller_agent::adapters::device_settings_json::{BootFilesFs, DeviceSettingsJson};
use controller_agent::boot_apply::{self, BootCtl, BootFiles, BootStore, Command, Unit};
use controller_agent::domain::device_settings::{self as ds, DeviceSettings, Loaded, Setting, UsbRole, UsbRoleStatus};
use controller_agent::ports::{DeviceCtl, DeviceSettingsStore};
use controller_agent::utils::config::{Config, DeviceSettingsConfig};
use controller_agent::utils::types::AppError;
use std::sync::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ---------- fakes ----------

struct FakeStore {
    settings: DeviceSettings,
}

impl DeviceSettingsStore for FakeStore {
    fn load(&self) -> Loaded {
        Loaded { settings: self.settings, image_id: "img".into(), write: false, quarantine: false, persistable: true, notes: vec![] }
    }
    fn save(&self, _: &str, _: &DeviceSettings) -> Result<(), AppError> {
        panic!("boot apply must not save through the store in these paths");
    }
}

impl BootStore for FakeStore {
    fn peek_mode(&self) -> UsbRole {
        self.settings.mode()
    }
}

#[derive(Default)]
struct FakeCtl {
    calls: Mutex<Vec<String>>,
    role: Option<UsbRole>,
    active: HashSet<&'static str>,
    fail: HashSet<&'static str>,
}

impl FakeCtl {
    fn rec(&self, s: impl Into<String>) {
        self.calls.lock().unwrap().push(s.into());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn res(&self, key: &'static str) -> Result<(), AppError> {
        if self.fail.contains(key) {
            Err(AppError::failed(format!("{key} failed")))
        } else {
            Ok(())
        }
    }
}

fn uname(u: Unit) -> &'static str {
    match u {
        Unit::Adbd => "adbd",
        Unit::UsbBind => "usb-bind",
        Unit::UartRoot => "uart-root",
        Unit::Getty => "getty",
    }
}

impl DeviceCtl for FakeCtl {
    fn usb_status(&self) -> Result<UsbRoleStatus, AppError> {
        self.rec("status");
        Ok(UsbRoleStatus { role: self.role, boot_role: None })
    }
    fn is_on(&self, s: Setting) -> Result<bool, AppError> {
        self.rec(format!("is_on {}", s.as_str()));
        Ok(self.active.contains(s.as_str()))
    }
    fn set_live(&self, s: Setting, on: bool) -> Result<(), AppError> {
        let key: &'static str = match (s, on) {
            (Setting::Adb, true) => "adb_on",
            (Setting::Adb, false) => "adb_off",
            (Setting::Uart, true) => "uart_on",
            (Setting::Uart, false) => "uart_off",
            (Setting::Ssh, true) => "ssh_on",
            (Setting::Ssh, false) => "ssh_off",
            (Setting::Mode, _) => "mode",
        };
        self.rec(key);
        self.res(key)
    }
    fn to_host(&self) -> Result<(), AppError> {
        self.rec("to_host");
        self.res("to_host")
    }
    fn to_device(&self) -> Result<(), AppError> {
        self.rec("to_device");
        self.res("to_device")
    }
    fn persist_boot_role(&self, _: UsbRole) -> Result<(), AppError> {
        panic!("boot apply never calls persist");
    }
    fn reboot(&self) -> Result<(), AppError> {
        panic!("boot apply never reboots");
    }
}

impl BootCtl for FakeCtl {
    fn is_active(&self, u: Unit) -> bool {
        self.rec(format!("active {}", uname(u)));
        self.active.contains(uname(u))
    }
    fn start(&self, u: Unit) -> Result<(), AppError> {
        let key = uname(u);
        self.rec(format!("start {key}"));
        self.res(key)
    }
}

#[derive(Default)]
struct FakeFiles {
    calls: Mutex<Vec<String>>,
    fail: bool,
}

impl BootFiles for FakeFiles {
    fn write_boot_role(&self, r: UsbRole) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("boot-role {}", r.as_str()));
        if self.fail { Err(AppError::failed("ro")) } else { Ok(()) }
    }
    fn set_ssh_off_flag(&self, present: bool) -> Result<(), AppError> {
        self.calls.lock().unwrap().push(format!("flag {present}"));
        if self.fail { Err(AppError::failed("ro")) } else { Ok(()) }
    }
    fn mark_applied(&self) -> Result<(), AppError> {
        self.calls.lock().unwrap().push("applied".into());
        Ok(())
    }
}

fn st(mode: UsbRole, ssh: bool, adb: bool, uart: bool) -> FakeStore {
    FakeStore { settings: DeviceSettings::new(mode, ssh, adb, uart) }
}

fn ctl(role: Option<UsbRole>) -> FakeCtl {
    FakeCtl { role, ..FakeCtl::default() }
}

fn started(c: &FakeCtl) -> Vec<String> {
    c.calls().into_iter().filter(|s| !s.starts_with("active") && !s.starts_with("is_on") && s != "status").collect()
}

// ---------- CLI ----------

#[test]
fn cli_parses_exactly_what_the_units_pass() {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(boot_apply::parse_args(&v(&["early"])), Ok(Command::Early));
    assert_eq!(boot_apply::parse_args(&v(&["late"])), Ok(Command::Late));
    assert_eq!(boot_apply::parse_args(&v(&["failsafe", "late"])), Ok(Command::FailsafeLate));
    assert_eq!(boot_apply::parse_args(&v(&["failsafe", "early", "timeout"])), Ok(Command::FailsafeEarly(Some("timeout".into()))));
    assert!(boot_apply::parse_args(&v(&["failsafe"])).is_err());
}

// ---------- early ----------

#[test]
fn early_mirrors_mode_and_sets_the_flag_from_ssh() {
    for (mode, ssh, role, flag) in [(UsbRole::Host, false, "host", true), (UsbRole::Device, true, "device", false), (UsbRole::Device, false, "device", true)] {
        let f = FakeFiles::default();
        assert_eq!(boot_apply::early(&st(mode, ssh, false, false), &f), 0);
        assert_eq!(*f.calls.lock().unwrap(), vec![format!("boot-role {role}"), format!("flag {flag}"), "applied".to_string()]);
    }
}

#[test]
fn early_fails_when_its_files_cannot_be_written_and_marks_nothing() {
    let f = FakeFiles { fail: true, ..FakeFiles::default() };
    assert_eq!(boot_apply::early(&st(UsbRole::Device, true, false, false), &f), 1);
    assert!(!f.calls.lock().unwrap().contains(&"applied".to_string()));
}

// ---------- failsafe early ----------

#[test]
fn failsafe_early_is_a_no_op_on_success() {
    let f = FakeFiles::default();
    assert_eq!(boot_apply::failsafe_early(Some("success"), &f), 0);
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn failsafe_early_restores_device_role_and_ssh_for_every_other_result() {
    for r in [Some("exit-code"), Some("timeout"), Some("signal"), Some("core-dump"), Some(""), Some("Success"), None] {
        let f = FakeFiles::default();
        assert_eq!(boot_apply::failsafe_early(r, &f), 0, "{r:?}");
        assert_eq!(*f.calls.lock().unwrap(), vec!["boot-role device".to_string(), "flag false".to_string()], "{r:?}");
    }
}

#[test]
fn failsafe_early_reports_failure_when_files_cannot_be_written() {
    let f = FakeFiles { fail: true, ..FakeFiles::default() };
    assert_eq!(boot_apply::failsafe_early(Some("exit-code"), &f), 1);
}

// ---------- late ----------

#[test]
fn late_device_adb_off_uart_off_ssh_on() {
    let c = ctl(Some(UsbRole::Device));
    assert_eq!(boot_apply::late(&st(UsbRole::Device, true, false, false), &c), 0);
    assert_eq!(started(&c), vec!["adb_off", "uart_off", "ssh_on"]);
}

#[test]
fn late_device_adb_on_uart_on_starts_the_persisted_pair() {
    let c = ctl(Some(UsbRole::Device));
    assert_eq!(boot_apply::late(&st(UsbRole::Device, true, true, true), &c), 0);
    assert_eq!(started(&c), vec!["adb_on", "uart_on", "ssh_on"]);
}

#[test]
fn late_host_never_starts_adbd_or_usb_bind() {
    let c = ctl(Some(UsbRole::Device));
    assert_eq!(boot_apply::late(&st(UsbRole::Host, true, true, false), &c), 0);
    let calls = started(&c);
    assert_eq!(calls, vec!["to_host", "uart_off", "ssh_on"]);
    assert!(!calls.iter().any(|s| s.starts_with("adb")));
}

#[test]
fn late_does_not_switch_a_role_that_already_matches() {
    let c = ctl(Some(UsbRole::Host));
    boot_apply::late(&st(UsbRole::Host, true, false, false), &c);
    assert!(!started(&c).contains(&"to_host".to_string()));
}

#[test]
fn late_ssh_off_closes_ssh_and_ssh_on_only_starts_a_stopped_socket() {
    let c = ctl(Some(UsbRole::Device));
    boot_apply::late(&st(UsbRole::Device, false, false, false), &c);
    assert!(started(&c).contains(&"ssh_off".to_string()));
    let mut c = ctl(Some(UsbRole::Device));
    c.active.insert("ssh");
    boot_apply::late(&st(UsbRole::Device, true, false, false), &c);
    assert!(!started(&c).iter().any(|s| s.starts_with("ssh")));
    let c = ctl(Some(UsbRole::Device));
    boot_apply::late(&st(UsbRole::Device, true, false, false), &c);
    assert!(started(&c).contains(&"ssh_on".to_string()));
}

#[test]
fn late_ssh_failure_is_not_a_unit_failure() {
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("ssh_off");
    assert_eq!(boot_apply::late(&st(UsbRole::Device, false, false, false), &c), 0);
}

#[test]
fn late_adbd_failure_falls_back_to_usb_bind_and_still_succeeds() {
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("adb_on");
    assert_eq!(boot_apply::late(&st(UsbRole::Device, true, true, false), &c), 0);
    assert_eq!(started(&c), vec!["adb_on", "adb_off", "uart_off", "ssh_on"]);
}

#[test]
fn late_fails_only_without_a_link_or_a_console() {
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("adb_off");
    assert_eq!(boot_apply::late(&st(UsbRole::Device, true, false, false), &c), 1);
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("uart_off");
    assert_eq!(boot_apply::late(&st(UsbRole::Device, true, false, false), &c), 1);
    // Host mode has no link requirement.
    let mut c = ctl(Some(UsbRole::Host));
    c.fail.insert("adb_off");
    assert_eq!(boot_apply::late(&st(UsbRole::Host, true, false, false), &c), 0);
}

// ---------- failsafe late ----------

#[test]
fn failsafe_late_device_nothing_running_starts_usb_bind_and_getty_only() {
    let c = ctl(Some(UsbRole::Device));
    assert_eq!(boot_apply::failsafe_late(&st(UsbRole::Device, true, true, true), &c), 0);
    assert_eq!(started(&c), vec!["start usb-bind", "start getty"]);
}

#[test]
fn failsafe_late_host_mode_never_starts_usb_bind() {
    let c = ctl(Some(UsbRole::Host));
    boot_apply::failsafe_late(&st(UsbRole::Host, true, false, false), &c);
    assert_eq!(started(&c), vec!["start getty"]);
}

#[test]
fn failsafe_late_live_host_role_blocks_usb_bind_even_if_persisted_device() {
    let c = ctl(Some(UsbRole::Host));
    boot_apply::failsafe_late(&st(UsbRole::Device, true, false, false), &c);
    assert!(!started(&c).contains(&"start usb-bind".to_string()));
}

#[test]
fn failsafe_late_leaves_running_surfaces_alone() {
    for running in [["adbd", "uart-root"], ["usb-bind", "getty"], ["adbd", "getty"]] {
        let mut c = ctl(Some(UsbRole::Device));
        c.active.extend(running);
        assert_eq!(boot_apply::failsafe_late(&st(UsbRole::Device, true, true, true), &c), 0);
        assert!(started(&c).is_empty(), "{running:?}: {:?}", c.calls());
    }
}

#[test]
fn failsafe_late_never_starts_adbd_or_uart_root_or_touches_ssh() {
    for mode in [UsbRole::Device, UsbRole::Host] {
        let c = ctl(Some(mode));
        boot_apply::failsafe_late(&st(mode, false, true, true), &c);
        for call in started(&c) {
            assert!(call == "start usb-bind" || call == "start getty", "{call}");
        }
    }
}

#[test]
fn failsafe_late_exit_is_nonzero_only_when_a_needed_start_failed() {
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("getty");
    assert_eq!(boot_apply::failsafe_late(&st(UsbRole::Device, true, false, false), &c), 1);
    let mut c = ctl(Some(UsbRole::Device));
    c.fail.insert("usb-bind");
    assert_eq!(boot_apply::failsafe_late(&st(UsbRole::Device, true, false, false), &c), 1);
}

// ---------- real adapters over a scratch dir ----------

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("boot-apply-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("var")).unwrap();
        std::fs::create_dir_all(p.join("run")).unwrap();
        Dir(p)
    }
    fn cfg(&self) -> DeviceSettingsConfig {
        let s = |p: &str| self.0.join(p).to_string_lossy().into_owned();
        DeviceSettingsConfig {
            settings_path: s("var/device-settings.json"),
            seed_path: s("seed.json"),
            boot_role_path: s("var/boot-role"),
            ssh_off_flag_path: s("run/ssh-off"),
            ..DeviceSettingsConfig::default()
        }
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SEED_ON: &str = r#"{"version":1,"image_id":"img-2","adb":true,"uart":true}"#;

#[test]
fn early_end_to_end_seeds_syncs_boot_role_and_flag() {
    let d = Dir::new("e2e");
    std::fs::write(d.0.join("seed.json"), SEED_ON).unwrap();
    std::fs::write(d.0.join("var/boot-role"), "host\n").unwrap();
    let cfg = d.cfg();
    let (store, files) = (DeviceSettingsJson::new(&cfg), BootFilesFs::new(&cfg));
    assert_eq!(boot_apply::early(&store, &files), 0);
    // adb seed on: device mode, legacy host overwritten.
    assert_eq!(std::fs::read_to_string(d.0.join("var/boot-role")).unwrap(), "device\n");
    assert!(!d.0.join("run/ssh-off").exists());
    assert!(std::fs::read_to_string(d.0.join("var/device-settings.json")).unwrap().contains("\"adb\":true"));
}

#[test]
fn early_end_to_end_ssh_off_and_host_persisted() {
    let d = Dir::new("e2e-off");
    std::fs::write(d.0.join("seed.json"), SEED_ON).unwrap();
    std::fs::write(d.0.join("var/device-settings.json"), ds::to_json("img-2", &DeviceSettings::new(UsbRole::Host, false, false, false))).unwrap();
    let cfg = d.cfg();
    assert_eq!(boot_apply::early(&DeviceSettingsJson::new(&cfg), &BootFilesFs::new(&cfg)), 0);
    assert_eq!(std::fs::read_to_string(d.0.join("var/boot-role")).unwrap(), "host\n");
    assert!(d.0.join("run/ssh-off").exists());
}

#[test]
fn early_end_to_end_on_a_dead_var_does_not_panic_and_keeps_ssh_reachable() {
    let d = Dir::new("e2e-dead");
    std::fs::write(d.0.join("seed.json"), SEED_ON).unwrap();
    std::fs::remove_dir_all(d.0.join("var")).unwrap();
    std::fs::write(d.0.join("var"), "a file where /var should be").unwrap();
    std::fs::write(d.0.join("run/ssh-off"), "").unwrap();
    let cfg = d.cfg();
    let code = boot_apply::early(&DeviceSettingsJson::new(&cfg), &BootFilesFs::new(&cfg));
    assert_eq!(code, 1, "boot-role cannot be written");
    // Fail-safe values: ssh on, so the flag is removed even though the rest failed.
    assert!(!d.0.join("run/ssh-off").exists());
    // The failsafe branch then also runs without panicking.
    assert_eq!(boot_apply::failsafe_early(Some("exit-code"), &BootFilesFs::new(&cfg)), 1);
}

#[test]
fn failsafe_peek_reads_mode_without_changing_files() {
    let d = Dir::new("peek");
    let cfg = d.cfg();
    let store = DeviceSettingsJson::new(&cfg);
    std::fs::write(d.0.join("var/device-settings.json"), "garbage").unwrap();
    assert_eq!(store.peek_mode(), UsbRole::Device);
    assert_eq!(std::fs::read_to_string(d.0.join("var/device-settings.json")).unwrap(), "garbage");
}

// ---------- layering: the early path never reaches a unit control ----------

fn src(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel)).unwrap()
}

#[test]
fn the_early_path_never_references_a_systemctl_adapter() {
    let text = src("boot_apply.rs");
    let begin = text.find("// early-path:begin").expect("begin marker");
    let end = text.find("// early-path:end").expect("end marker");
    let early = &text[begin..end];
    assert!(early.contains("pub fn early(") && early.contains("pub fn failsafe_early("), "markers must wrap both functions");
    for bad in ["systemctl", "Systemctl", "DeviceCtl", "BootCtl", "Command::new", "process::", "device_ctl_cli", "Unit::", "usb_role", "set_live"] {
        assert!(!early.contains(bad), "early path mentions {bad}");
    }
    // The early entry points take only the store and the file writer.
    assert!(early.contains("pub fn early(store: &dyn DeviceSettingsStore, files: &dyn BootFiles)"));
    assert!(early.contains("pub fn failsafe_early(result: Option<&str>, files: &dyn BootFiles)"));
}

#[test]
fn the_file_adapters_spawn_no_process() {
    let text = src("adapters/device_settings_json.rs");
    let prod = text.split("#[cfg(test)]").next().unwrap();
    for bad in ["Command::new", "process::", "systemctl", "\"sh\"", "\"-c\""] {
        assert!(!prod.contains(bad), "device_settings_json.rs mentions {bad}");
    }
}

#[test]
fn main_dispatches_before_config_load_and_early_builds_no_ctl() {
    let m = src("main.rs");
    let dispatch = m.find("\"apply-device-settings\"").unwrap();
    let load = m.find("Config::load()").unwrap();
    assert!(dispatch < load, "apply-device-settings must be dispatched before Config::load");
    let f = &m[m.find("fn apply_device_settings").unwrap()..m.find("fn main()").unwrap()];
    assert!(!f.contains("Config::load"));
    // DeviceCtlCli is built only in the two late arms.
    assert_eq!(f.matches("DeviceCtlCli::new").count(), 2);
    assert!(!f.split("Command::Early").nth(1).unwrap().split("Command::Late").next().unwrap().contains("DeviceCtlCli"));
}

// ---------- constants pinned to the shipped config ----------

#[test]
fn compiled_in_constants_equal_the_shipped_controller_toml() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/controller.toml");
    let loaded = Config::load_from(&path).unwrap();
    let d = DeviceSettingsConfig::default();
    assert_eq!(loaded.config.device_settings, d);
    assert_eq!(d.systemctl_command, ds::SYSTEMCTL_COMMAND);
    assert_eq!(d.usb_role_command, ds::USB_ROLE_COMMAND);
    assert_eq!(d.adb_unit, ds::ADB_UNIT);
    assert_eq!(d.adb_disabled_unit, ds::USB_BIND_UNIT);
    assert_eq!(d.uart_unit, ds::UART_UNIT);
    assert_eq!(d.uart_disabled_unit, ds::GETTY_UNIT);
    assert_eq!(d.ssh_socket_unit, ds::SSH_SOCKET_UNIT);
    assert_eq!(d.ssh_instance_glob, ds::SSH_INSTANCE_GLOB);
    assert_eq!(d.ssh_off_flag_path, ds::SSH_OFF_FLAG_PATH);
    assert_eq!(d.settings_path, ds::SETTINGS_PATH);
    assert_eq!(d.seed_path, ds::SEED_PATH);
    assert_eq!(d.boot_role_path, ds::LEGACY_BOOT_ROLE_PATH);
}

#[test]
fn unit_files_call_the_subcommands_this_binary_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/overlay/recipes-extended/thing-revived-device-apply/files");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines() {
            let Some(rest) = line.split("controller-agent apply-device-settings ").nth(1) else { continue };
            if line.trim_start().starts_with('#') {
                continue;
            }
            let args: Vec<String> = rest.split_whitespace().map(|a| if a == "$SERVICE_RESULT" { "exit-code".into() } else { a.to_string() }).collect();
            assert!(boot_apply::parse_args(&args).is_ok(), "{line}");
            seen += 1;
        }
    }
    assert_eq!(seen, 4, "units A (2 lines), B and F");
}
