//! Enforces the module layering of the crate: device libraries (`zbus`,
//! `evdev`) may only appear in `adapters/` (and `utils/`, which has none
//! today); `domain/` must not reach up into `services/`, `app/` or `adapters/`.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn offenders(files: &[PathBuf], bad: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if bad(line.trim_start()) {
                out.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    out
}

fn device_lib(t: &str) -> bool {
    t.starts_with("use zbus") || t.starts_with("use evdev") || t.contains("zbus::") || t.contains("evdev::")
}

#[test]
fn only_adapters_and_utils_touch_zbus_or_evdev() {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(src()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name == "adapters" || name == "utils" {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, &mut files);
        } else if name.ends_with(".rs") {
            files.push(path);
        }
    }
    let scanned_layers: Vec<String> = ["domain", "app", "services"].iter().map(|l| src().join(l).display().to_string()).collect();
    for layer in &scanned_layers {
        assert!(files.iter().any(|f| f.starts_with(layer)), "{layer} not scanned: layering test is vacuous");
    }
    let bad = offenders(&files, device_lib);
    assert!(bad.is_empty(), "layering violation(s):\n{}", bad.join("\n"));
}

#[test]
fn domain_does_not_depend_on_upper_layers() {
    let mut files = Vec::new();
    rust_files(&src().join("domain"), &mut files);
    assert!(!files.is_empty());
    let bad = offenders(&files, |t| {
        ["crate::adapters", "crate::services", "crate::app", "crate::ports", "crate::protocol"].iter().any(|p| t.contains(p))
    });
    assert!(bad.is_empty(), "domain reaches into upper layers:\n{}", bad.join("\n"));
}

/// Every `.rs` file under `src/` except those in `adapters/`.
fn non_adapter_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(src()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name == "adapters" {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, &mut files);
        } else if name.ends_with(".rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn only_adapters_spawn_processes() {
    let files = non_adapter_files();
    assert!(files.iter().any(|f| f.ends_with("services/users.rs")), "users service not scanned: test is vacuous");
    let bad = offenders(&files, |t| {
        !t.starts_with("//") && (t.contains("process::Command") || t.contains("Command::new") || t.contains("use std::process"))
    });
    assert!(bad.is_empty(), "process spawning outside adapters/:\n{}", bad.join("\n"));
}

/// The text of a source file before its `#[cfg(test)]` module.
fn production_text(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.split("#[cfg(test)]").next().unwrap().to_string()
}

#[test]
fn only_the_settings_file_adapter_and_its_constants_name_the_persisted_paths() {
    // The device settings service and the live control adapter never open a
    // path under `/var` or `/etc`: persistence goes through the store port,
    // whose one implementation is `device_settings_json.rs`. Paths exist only
    // as compiled-in constants (`domain/device_settings.rs`), their config
    // defaults (`utils/config.rs`) and the adapters that read the files.
    let service = src().join("services/device_settings.rs");
    let ctl = src().join("adapters/device_ctl_cli.rs");
    for f in [&service, &ctl] {
        assert!(f.is_file(), "{} missing: test is vacuous", f.display());
        let bad: Vec<String> = production_text(f)
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && (l.contains("\"/var") || l.contains("\"/etc")))
            .map(String::from)
            .collect();
        assert!(bad.is_empty(), "{} names a persistent path:\n{}", f.display(), bad.join("\n"));
    }
    // The service does no file or process I/O of its own.
    let text = production_text(&service);
    for banned in ["std::fs", "std::process", "std::net", "crate::adapters"] {
        assert!(!text.contains(banned), "services/device_settings.rs production code uses {banned}");
    }
    // Only the settings file adapter, the shared domain constants, the config
    // defaults and the startup repair may mention the settings file name.
    let mut files = Vec::new();
    rust_files(&src(), &mut files);
    let allowed = ["adapters/device_settings_json.rs", "domain/device_settings.rs", "utils/config.rs", "adapters/device_ctl_cli.rs", "app/startup.rs"];
    let bad = offenders(
        &files.iter().filter(|f| !allowed.iter().any(|a| f.ends_with(a))).cloned().collect::<Vec<_>>(),
        |t| !t.starts_with("//") && t.contains("device-settings.json"),
    );
    assert!(bad.is_empty(), "the settings file is named outside its adapter:\n{}", bad.join("\n"));
}

#[test]
fn nothing_mentions_the_removed_adb_marker_file() {
    let mut files = Vec::new();
    rust_files(&src(), &mut files);
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"), &mut files);
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/layering.rs");
    files.retain(|f| f != &this);
    let marker = ["usb", "debugging", "enabled"].join("-");
    let bad = offenders(&files, |t| t.contains(&marker));
    assert!(bad.is_empty(), "the removed marker file is still referenced:\n{}", bad.join("\n"));
    let toml = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/controller.toml")).unwrap();
    assert!(!toml.contains(&marker));
}

#[test]
fn the_early_boot_path_never_spawns_systemctl() {
    // The early unit and its failsafe run before the units they would start
    // exist; a `systemctl` call there can deadlock the boot. The marked
    // region of `boot_apply.rs` holds both entry points.
    let text = std::fs::read_to_string(src().join("boot_apply.rs")).unwrap();
    let begin = text.find("// early-path:begin").expect("begin marker");
    let end = text.find("// early-path:end").expect("end marker");
    let early = &text[begin..end];
    for banned in ["systemctl", "Systemctl", "Command::new", "process::", "DeviceCtl", "BootCtl"] {
        assert!(!early.contains(banned), "the early path uses {banned}");
    }
    // The file writer the early path uses has no process spawning either.
    let json = production_text(&src().join("adapters/device_settings_json.rs"));
    for banned in ["Command::new", "process::", "systemctl"] {
        assert!(!json.contains(banned), "device_settings_json.rs production code uses {banned}");
    }
}

#[test]
fn account_adapters_never_use_a_shell() {
    let adapters = src().join("adapters");
    let files: Vec<PathBuf> = [
        "accounts_shadow.rs",
        "accounts_db.rs",
        "authorized_keys_fs.rs",
        "os_entropy.rs",
        "ssh_keygen.rs",
        "unix_chkpwd.rs",
    ].iter().map(|n| adapters.join(n)).collect();
    for f in &files {
        assert!(f.is_file(), "{} missing: test is vacuous", f.display());
    }
    // Test modules are exempt: they may name a fake runner's argv.
    let bad = offenders(&files, |t| {
        let code = t.split("//").next().unwrap_or("");
        code.contains("\"sh\"") || code.contains("\"/bin/sh\"") || code.contains("\"bash\"") || code.contains("\"-c\"")
    });
    assert!(bad.is_empty(), "shell use in an account adapter:\n{}", bad.join("\n"));
}

#[test]
fn the_user_services_depend_only_on_ports_and_domain() {
    for name in ["services/users.rs", "services/accounts.rs"] {
        let text = std::fs::read_to_string(src().join(name)).unwrap();
        assert!(text.contains("pub mod testing"), "{name}: the production-code split below would be vacuous");
        let production = text.split("pub mod testing").next().unwrap();
        for banned in ["crate::adapters", "std::process", "std::fs", "std::net", "std::env"] {
            assert!(!production.contains(banned), "{name} production code uses {banned}");
        }
    }
}

#[test]
fn account_adapters_pass_no_password_through_argv_or_the_environment() {
    // Every `CommandSpec` that carries a secret builds it into `stdin`; the only
    // other callers of `.arg`/`.env` are the runner itself.
    let adapters = src().join("adapters");
    for name in ["unix_chkpwd.rs", "ssh_keygen.rs", "authorized_keys_fs.rs", "accounts_db.rs"] {
        let text = std::fs::read_to_string(adapters.join(name)).unwrap();
        let production = text.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("Command::new") && !production.contains(".env("), "{name} spawns or sets env itself; it must go through the Runner");
    }
    let chk = std::fs::read_to_string(adapters.join("unix_chkpwd.rs")).unwrap();
    assert!(chk.contains("spec.stdin"), "unix_chkpwd must hand the password over on stdin");
    let code: String = chk.split("#[cfg(test)]").next().unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    assert!(code.contains("\"nonull\"") && !code.contains("nullok"), "unix_chkpwd must use nonull, never nullok");
}
