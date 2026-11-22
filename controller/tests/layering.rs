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
