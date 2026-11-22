//! Enforces the module layering of the crate: `domain/` and `app/` must not
//! depend on the device libraries (`zbus`, `evdev`); only adapters (and, until
//! they are split, the legacy device modules) may.

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

#[test]
fn domain_and_app_do_not_import_zbus_or_evdev() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for layer in ["domain", "app"] {
        rust_files(&src.join(layer), &mut files);
    }
    assert!(!files.is_empty(), "no files scanned: layering test is vacuous");
    let mut offenders = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("use zbus") || t.starts_with("use evdev") || t.contains("zbus::") || t.contains("evdev::") {
                offenders.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(offenders.is_empty(), "layering violation(s):\n{}", offenders.join("\n"));
}
