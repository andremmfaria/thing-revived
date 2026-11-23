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
