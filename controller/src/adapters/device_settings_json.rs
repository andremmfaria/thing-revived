//! `DeviceSettingsStore` over the persisted JSON file, plus the boot-time
//! file writes (`BootFiles`) the early unit needs: the legacy `boot-role`
//! mirror and the `ssh-off` flag. All parsing and every decision live in
//! `domain::device_settings`; this module only reads the three input files,
//! performs the write and quarantine the loader asks for, and never panics
//! on an unreadable or read-only `/var`.
//!
//! Atomic write: temp file in the same directory (0600), write, fsync,
//! rename over the target, fsync the directory. A power cut leaves the old
//! or the new file, never a truncated one; a stale temp file is replaced.
//!
//! Locking: `load` and `save` take an exclusive lock on
//! `<runtime dir>/device-settings.lock` (the directory of the ssh-off flag)
//! so the controller's startup fallback cannot race the early unit while it
//! writes. The lock is best effort: if it cannot be taken within a few
//! seconds the operation proceeds without it, because a stuck lock must not
//! block the boot.

use crate::boot_apply::{BootFiles, BootStore};
use crate::domain::device_settings as ds;
use crate::domain::device_settings::{DeviceSettings, Loaded, StoredFile, UsbRole};
use crate::ports::DeviceSettingsStore;
use crate::utils::config::DeviceSettingsConfig;
use crate::utils::types::AppError;
use log::{info, warn};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Name of the lock file inside the runtime directory.
pub const LOCK_FILE_NAME: &str = "device-settings.lock";
/// Name of the marker the early unit writes after a clean apply; the
/// controller's startup fallback is gated on its absence.
pub const APPLIED_FILE_NAME: &str = "device-settings.applied";

const LOCK_WAIT: Duration = Duration::from_secs(5);

pub struct DeviceSettingsJson {
    settings: PathBuf,
    seed: PathBuf,
    boot_role: PathBuf,
    lock: PathBuf,
}

impl DeviceSettingsJson {
    pub fn new(cfg: &DeviceSettingsConfig) -> DeviceSettingsJson {
        DeviceSettingsJson {
            settings: PathBuf::from(&cfg.settings_path),
            seed: PathBuf::from(&cfg.seed_path),
            boot_role: PathBuf::from(&cfg.boot_role_path),
            lock: runtime_dir(cfg).join(LOCK_FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.settings
    }

    fn lock_guard(&self) -> Option<File> {
        if let Some(dir) = self.lock.parent() {
            let _ = fs::DirBuilder::new().recursive(true).mode(0o755).create(dir);
        }
        let f = OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(&self.lock).ok()?;
        let start = Instant::now();
        loop {
            match f.try_lock() {
                Ok(()) => return Some(f),
                Err(_) if start.elapsed() < LOCK_WAIT => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => {
                    warn!("device-settings: lock {} busy for {LOCK_WAIT:?}, continuing without it", self.lock.display());
                    return None;
                }
            }
        }
    }

    fn write_settings(&self, image_id: &str, settings: &DeviceSettings) -> std::io::Result<()> {
        atomic_write(&self.settings, ds::to_json(image_id, settings).as_bytes(), 0o600, true)
    }

    /// Renames the bad file to `<name>.corrupt` (overwriting the previous
    /// one) and fsyncs the directory.
    fn quarantine(&self) -> std::io::Result<()> {
        let mut name = self.settings.file_name().unwrap_or_default().to_os_string();
        name.push(".corrupt");
        let parent = self.settings.parent().unwrap_or_else(|| Path::new("."));
        fs::rename(&self.settings, parent.join(name))?;
        fsync_dir(parent)
    }
}

enum Read {
    Missing,
    Unreadable,
    Present(String),
}

/// A missing file is first boot; any other read error (a directory in the
/// way, `/var` not mounted, EIO) is unreadable. Non-UTF-8 content is still
/// present: it fails to parse and is quarantined as corrupt.
fn read_state(path: &Path) -> Read {
    match fs::read(path) {
        Ok(bytes) => Read::Present(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Unreadable,
    }
}

/// Optional input file (seed, legacy boot-role): `None` on any failure.
fn read_optional(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The persisted mode for the failsafe unit: device when the file is
/// missing, unreadable or corrupt. Reads only, never writes.
impl BootStore for DeviceSettingsJson {
    fn peek_mode(&self) -> UsbRole {
        match read_state(&self.settings) {
            Read::Missing | Read::Unreadable => UsbRole::Device,
            Read::Present(t) => ds::tolerant_mode(StoredFile::Present(&t)),
        }
    }
}

impl DeviceSettingsStore for DeviceSettingsJson {
    fn load(&self) -> Loaded {
        let _lock = self.lock_guard();
        let stored = read_state(&self.settings);
        let seed = read_optional(&self.seed);
        let legacy = read_optional(&self.boot_role);
        let state = match &stored {
            Read::Missing => StoredFile::Missing,
            Read::Unreadable => StoredFile::Unreadable,
            Read::Present(t) => StoredFile::Present(t),
        };
        let mut loaded = ds::load(state, seed.as_deref(), legacy.as_deref());
        for note in &loaded.notes {
            if note.is_warn() {
                warn!("{}", note.message());
            } else {
                info!("{}", note.message());
            }
        }
        if loaded.quarantine {
            if let Err(e) = self.quarantine() {
                warn!("device-settings: could not keep the corrupt file as .corrupt: {e}");
            }
        }
        if loaded.write {
            if let Err(e) = self.write_settings(&loaded.image_id, &loaded.settings) {
                // Cannot persist: do not pretend. Fail-safe values stay in
                // memory and every set answers persist_failed.
                warn!("device-settings: could not write {}: {e}, using fail-safe values in memory", self.settings.display());
                loaded.settings = DeviceSettings::failsafe();
                loaded.persistable = false;
            }
        }
        loaded
    }

    fn save(&self, image_id: &str, settings: &DeviceSettings) -> Result<(), AppError> {
        let _lock = self.lock_guard();
        self.write_settings(image_id, settings).map_err(|e| AppError::failed(format!("write {}: {e}", self.settings.display())))
    }
}

/// Atomic replace with mode `mode`. `private_dir` creates a missing parent
/// as 0700 (the settings directory), otherwise 0755.
pub fn atomic_write(path: &Path, data: &[u8], mode: u32, private_dir: bool) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new().recursive(true).mode(if private_dir { 0o700 } else { 0o755 }).create(parent)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);

    let result = (|| {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        // `mode` only applies on creation; fix up a leftover with other bits.
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fsync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

fn runtime_dir(cfg: &DeviceSettingsConfig) -> PathBuf {
    Path::new(&cfg.ssh_off_flag_path).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// The early unit's plain file writes: `boot-role` and the `ssh-off` flag,
/// no process involved.
pub struct BootFilesFs {
    boot_role: PathBuf,
    ssh_off_flag: PathBuf,
    applied: PathBuf,
}

impl BootFilesFs {
    pub fn new(cfg: &DeviceSettingsConfig) -> BootFilesFs {
        BootFilesFs {
            boot_role: PathBuf::from(&cfg.boot_role_path),
            ssh_off_flag: PathBuf::from(&cfg.ssh_off_flag_path),
            applied: runtime_dir(cfg).join(APPLIED_FILE_NAME),
        }
    }
}

fn io_err(what: &Path, e: std::io::Error) -> AppError {
    AppError::failed(format!("{}: {e}", what.display()))
}

impl BootFiles for BootFilesFs {
    /// Same content the stock `superbird-usb-role persist` writes (role and
    /// a newline). Skipped when the file already holds exactly that.
    fn write_boot_role(&self, role: UsbRole) -> Result<(), AppError> {
        let want = format!("{}\n", role.as_str());
        if fs::read_to_string(&self.boot_role).is_ok_and(|have| have == want) {
            return Ok(());
        }
        atomic_write(&self.boot_role, want.as_bytes(), 0o644, false).map_err(|e| io_err(&self.boot_role, e))
    }

    fn set_ssh_off_flag(&self, present: bool) -> Result<(), AppError> {
        if present {
            create_flag(&self.ssh_off_flag)
        } else {
            remove_flag(&self.ssh_off_flag)
        }
    }

    fn mark_applied(&self) -> Result<(), AppError> {
        create_flag(&self.applied)
    }
}

/// Creates an empty flag file (and its directory). Shared with the live
/// ssh toggle in `device_ctl_cli`.
pub fn create_flag(path: &Path) -> Result<(), AppError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new().recursive(true).mode(0o755).create(parent).map_err(|e| io_err(parent, e))?;
    OpenOptions::new().write(true).create(true).truncate(false).mode(0o644).open(path).map(|_| ()).map_err(|e| io_err(path, e))
}

/// Removes a flag file; an absent file is success.
pub fn remove_flag(path: &Path) -> Result<(), AppError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    const SEED_OFF: &str = r#"{"version":1,"image_id":"img-2","adb":false,"uart":false}"#;
    const SEED_ON: &str = r#"{"version":1,"image_id":"img-2","adb":true,"uart":true}"#;

    struct Rig {
        dir: PathBuf,
        store: DeviceSettingsJson,
        cfg: DeviceSettingsConfig,
    }

    impl Rig {
        fn new(name: &str) -> Rig {
            let dir = std::env::temp_dir().join(format!("controller-dsjson-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("var")).unwrap();
            fs::create_dir_all(dir.join("run")).unwrap();
            let cfg = DeviceSettingsConfig {
                settings_path: dir.join("var/device-settings.json").to_string_lossy().into_owned(),
                seed_path: dir.join("seed.json").to_string_lossy().into_owned(),
                boot_role_path: dir.join("var/boot-role").to_string_lossy().into_owned(),
                ssh_off_flag_path: dir.join("run/ssh-off").to_string_lossy().into_owned(),
                ..DeviceSettingsConfig::default()
            };
            Rig { store: DeviceSettingsJson::new(&cfg), dir, cfg }
        }
        fn settings(&self) -> PathBuf {
            self.dir.join("var/device-settings.json")
        }
        fn seed(&self, text: &str) {
            fs::write(self.dir.join("seed.json"), text).unwrap();
        }
        fn put(&self, text: &str) {
            fs::write(self.settings(), text).unwrap();
        }
        fn read(&self) -> String {
            fs::read_to_string(self.settings()).unwrap()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn mode_bits(p: &Path) -> u32 {
        fs::metadata(p).unwrap().mode() & 0o777
    }

    #[test]
    fn missing_file_seeds_and_writes_0600() {
        let r = Rig::new("seed");
        r.seed(SEED_ON);
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::new(UsbRole::Device, true, true, true));
        assert!(l.persistable);
        assert_eq!(r.read(), ds::to_json("img-2", &l.settings));
        assert_eq!(mode_bits(&r.settings()), 0o600);
        // A second load of what was written changes nothing.
        let again = r.store.load();
        assert_eq!(again.settings, l.settings);
        assert!(!again.write);
    }

    #[test]
    fn missing_file_honours_a_legacy_host_boot_role() {
        let r = Rig::new("legacy");
        r.seed(SEED_OFF);
        fs::write(r.dir.join("var/boot-role"), "host\n").unwrap();
        assert_eq!(r.store.load().settings.mode(), UsbRole::Host);
    }

    #[test]
    fn missing_file_and_missing_seed_writes_failsafe_with_empty_id() {
        let r = Rig::new("noseed");
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert_eq!(r.read(), ds::to_json("", &DeviceSettings::failsafe()));
    }

    #[test]
    fn missing_state_directory_is_created_private() {
        let r = Rig::new("mkdir");
        r.seed(SEED_OFF);
        let cfg = DeviceSettingsConfig {
            settings_path: r.dir.join("var/newdir/device-settings.json").to_string_lossy().into_owned(),
            ..r.cfg.clone()
        };
        let store = DeviceSettingsJson::new(&cfg);
        assert!(store.load().persistable);
        assert_eq!(mode_bits(&r.dir.join("var/newdir")), 0o700);
    }

    #[test]
    fn corrupt_variants_are_quarantined_once_and_replaced_by_failsafe() {
        let good = ds::to_json("img-2", &DeviceSettings::new(UsbRole::Device, false, true, true));
        let cases = [
            ("empty", String::new()),
            ("truncated", good[..good.len() - 12].to_string()),
            ("garbage", "garbage".to_string()),
            ("wrongtype", r#"{"version":1,"image_id":"img-2","ssh":"on"}"#.to_string()),
            ("badmode", r#"{"version":1,"image_id":"img-2","mode":"otg"}"#.to_string()),
            ("badversion", r#"{"version":9}"#.to_string()),
            ("array", "[1,2]".to_string()),
        ];
        for (name, text) in cases {
            let r = Rig::new(&format!("corrupt-{name}"));
            r.seed(SEED_ON);
            r.put(&text);
            let l = r.store.load();
            assert_eq!(l.settings, DeviceSettings::failsafe(), "{name}");
            assert!(l.persistable, "{name}");
            assert_eq!(fs::read_to_string(r.dir.join("var/device-settings.json.corrupt")).unwrap(), text, "{name}");
            // Fail-safe values written with the seed id so the next boot does not reseed adb on.
            assert_eq!(r.read(), ds::to_json("img-2", &DeviceSettings::failsafe()), "{name}");
            let again = r.store.load();
            assert_eq!(again.settings, DeviceSettings::failsafe(), "{name}");
        }
    }

    #[test]
    fn a_second_corrupt_file_overwrites_the_first_corrupt_copy() {
        let r = Rig::new("corrupt-twice");
        r.seed(SEED_OFF);
        r.put("first-bad");
        r.store.load();
        r.put("second-bad");
        r.store.load();
        assert_eq!(fs::read_to_string(r.dir.join("var/device-settings.json.corrupt")).unwrap(), "second-bad");
    }

    #[test]
    fn non_utf8_content_is_corrupt_not_unreadable() {
        let r = Rig::new("utf8");
        r.seed(SEED_OFF);
        fs::write(r.settings(), [0xff, 0xfe, 0x00]).unwrap();
        let l = r.store.load();
        assert!(l.persistable);
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert!(r.dir.join("var/device-settings.json.corrupt").exists());
    }

    #[test]
    fn host_with_adb_true_is_forced_off_and_rewritten() {
        let r = Rig::new("host-adb");
        r.seed(SEED_ON);
        r.put(r#"{"version":1,"image_id":"img-2","mode":"host","ssh":false,"adb":true,"uart":true}"#);
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::new(UsbRole::Host, false, false, true));
        assert_eq!(r.read(), ds::to_json("img-2", &l.settings));
        assert!(!r.dir.join("var/device-settings.json.corrupt").exists());
    }

    #[test]
    fn a_changed_image_id_reseeds_and_stores_the_new_id() {
        let r = Rig::new("reseed");
        r.seed(SEED_OFF);
        r.put(r#"{"version":1,"image_id":"img-1","mode":"host","ssh":false,"adb":false,"uart":true}"#);
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::new(UsbRole::Host, false, false, false));
        assert_eq!(r.read(), ds::to_json("img-2", &l.settings));
    }

    #[test]
    fn valid_file_with_unusable_seed_is_left_untouched() {
        let r = Rig::new("noseed-valid");
        let text = ds::to_json("img-1", &DeviceSettings::new(UsbRole::Device, true, true, false));
        r.put(&text);
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::new(UsbRole::Device, true, true, false));
        assert_eq!(r.read(), text);
    }

    #[test]
    fn an_unreadable_store_is_failsafe_in_memory_and_never_panics() {
        // A directory where the file should be: the read fails with something other than NotFound.
        let r = Rig::new("unreadable-dir");
        r.seed(SEED_ON);
        fs::create_dir(r.settings()).unwrap();
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert!(!l.persistable);
        assert!(r.store.save("img-2", &l.settings).is_err());
        assert!(r.settings().is_dir(), "the directory in the way is untouched");

        // A regular file where the directory should be (ENOTDIR on every access, also as root).
        let r = Rig::new("unreadable-notdir");
        r.seed(SEED_ON);
        fs::remove_dir_all(r.dir.join("var")).unwrap();
        fs::write(r.dir.join("var"), "i am a file").unwrap();
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::failsafe());
        assert!(!l.persistable);
        assert!(r.store.save("img-2", &l.settings).is_err());
        assert_eq!(fs::read_to_string(r.dir.join("var")).unwrap(), "i am a file");
    }

    #[test]
    fn a_failed_first_boot_write_falls_back_to_failsafe_and_not_persistable() {
        // Read-only /var simulated as a missing file (NotFound) whose directory cannot be created: /proc.
        let r = Rig::new("ro-first-boot");
        r.seed(SEED_ON);
        let cfg = DeviceSettingsConfig { settings_path: "/proc/thing-revived-test/device-settings.json".into(), ..r.cfg.clone() };
        let store = DeviceSettingsJson::new(&cfg);
        let l = store.load();
        assert_eq!(l.settings, DeviceSettings::failsafe(), "seed adb/uart on must not be used when nothing can be persisted");
        assert!(!l.persistable);
    }

    #[test]
    fn a_read_only_directory_fails_the_write_without_panicking() {
        // Effective only as a non-root user; as root the write succeeds and the test is a no-op.
        let r = Rig::new("ro-dir");
        r.seed(SEED_ON);
        let ro = r.dir.join("var");
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();
        let l = r.store.load();
        let is_root = fs::File::create(ro.join("probe")).is_ok();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o700)).unwrap();
        if !is_root {
            assert_eq!(l.settings, DeviceSettings::failsafe());
            assert!(!l.persistable);
        }
    }

    #[test]
    fn a_leftover_temp_file_from_a_power_cut_is_ignored_and_replaced() {
        let r = Rig::new("tmp-left");
        r.seed(SEED_OFF);
        let good = ds::to_json("img-2", &DeviceSettings::new(UsbRole::Host, false, false, true));
        r.put(&good);
        let tmp = r.dir.join("var/device-settings.json.tmp");
        fs::write(&tmp, r#"{"version":1,"image_id":"img-2","mode":"dev"#).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
        // The target is intact, so load ignores the half written temp.
        let l = r.store.load();
        assert_eq!(l.settings, DeviceSettings::new(UsbRole::Host, false, false, true));
        assert_eq!(r.read(), good);
        // The next save replaces it and ends with no temp file and 0600.
        let next = DeviceSettings::new(UsbRole::Device, true, true, false);
        r.store.save("img-2", &next).unwrap();
        assert_eq!(r.read(), ds::to_json("img-2", &next));
        assert!(!tmp.exists());
        assert_eq!(mode_bits(&r.settings()), 0o600);
    }

    #[test]
    fn a_power_cut_before_the_first_rename_leaves_only_a_temp_and_seeds_again() {
        let r = Rig::new("tmp-first");
        r.seed(SEED_OFF);
        fs::write(r.dir.join("var/device-settings.json.tmp"), "half").unwrap();
        let l = r.store.load();
        assert!(l.persistable);
        assert_eq!(r.read(), ds::to_json("img-2", &l.settings));
        assert!(!r.dir.join("var/device-settings.json.tmp").exists());
    }

    #[test]
    fn save_replaces_the_file_with_the_plan_shape_and_widens_nothing() {
        let r = Rig::new("save");
        r.put("old");
        fs::set_permissions(r.settings(), fs::Permissions::from_mode(0o644)).unwrap();
        let s = DeviceSettings::new(UsbRole::Host, true, true, true);
        r.store.save("img-9", &s).unwrap();
        assert_eq!(r.read(), "{\"version\":1,\"image_id\":\"img-9\",\"mode\":\"host\",\"ssh\":true,\"adb\":false,\"uart\":true}\n");
        // rename replaced the inode, so the mode is the temp file's 0600.
        assert_eq!(mode_bits(&r.settings()), 0o600);
    }

    #[test]
    fn peek_mode_reads_without_writing() {
        let r = Rig::new("peek");
        assert_eq!(r.store.peek_mode(), UsbRole::Device);
        assert!(!r.settings().exists());
        r.put(&ds::to_json("x", &DeviceSettings::new(UsbRole::Host, true, false, false)));
        assert_eq!(r.store.peek_mode(), UsbRole::Host);
        r.put("garbage");
        assert_eq!(r.store.peek_mode(), UsbRole::Device);
        assert_eq!(r.read(), "garbage");
        assert!(!r.dir.join("var/device-settings.json.corrupt").exists());
    }

    #[test]
    fn boot_files_write_the_stock_boot_role_content_and_skip_equal_writes() {
        let r = Rig::new("bootfiles");
        let files = BootFilesFs::new(&r.cfg);
        files.write_boot_role(UsbRole::Host).unwrap();
        assert_eq!(fs::read_to_string(r.dir.join("var/boot-role")).unwrap(), "host\n");
        let inode = fs::metadata(r.dir.join("var/boot-role")).unwrap().ino();
        files.write_boot_role(UsbRole::Host).unwrap();
        assert_eq!(fs::metadata(r.dir.join("var/boot-role")).unwrap().ino(), inode, "equal content is not rewritten");
        files.write_boot_role(UsbRole::Device).unwrap();
        assert_eq!(fs::read_to_string(r.dir.join("var/boot-role")).unwrap(), "device\n");
    }

    #[test]
    fn ssh_off_flag_is_created_and_removed_idempotently() {
        let r = Rig::new("flag");
        let files = BootFilesFs::new(&r.cfg);
        let flag = r.dir.join("run/ssh-off");
        files.set_ssh_off_flag(false).unwrap();
        assert!(!flag.exists());
        files.set_ssh_off_flag(true).unwrap();
        files.set_ssh_off_flag(true).unwrap();
        assert!(flag.exists());
        files.set_ssh_off_flag(false).unwrap();
        files.set_ssh_off_flag(false).unwrap();
        assert!(!flag.exists());
        files.mark_applied().unwrap();
        assert!(r.dir.join("run").join(APPLIED_FILE_NAME).exists());
    }

    #[test]
    fn boot_file_errors_are_errors_not_panics() {
        let r = Rig::new("bootfiles-err");
        fs::remove_dir_all(r.dir.join("var")).unwrap();
        fs::write(r.dir.join("var"), "file").unwrap();
        let files = BootFilesFs::new(&r.cfg);
        assert!(files.write_boot_role(UsbRole::Device).is_err());
    }
}
