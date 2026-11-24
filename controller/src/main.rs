mod provision;

use controller_agent::adapters::device_ctl_cli::DeviceCtlCli;
use controller_agent::adapters::device_settings_json::{BootFilesFs, DeviceSettingsJson};
use controller_agent::boot_apply::{self, Command};
use controller_agent::utils::config::{Config, DeviceSettingsConfig, LoggingConfig, EX_CONFIG};
use controller_agent::utils::logging;

/// `controller-agent apply-device-settings early|late|failsafe ...`, run by
/// the three device settings units. It uses compiled-in unit names and paths
/// (`DeviceSettingsConfig::default()`, equal to the `domain::device_settings`
/// constants) and never loads `controller.toml`, so a stale config cannot stop
/// the boot. `early` and `failsafe early` build no unit control at all.
fn apply_device_settings(args: &[String]) -> i32 {
    logging::init(&LoggingConfig::default());
    let cmd = match boot_apply::parse_args(args) {
        Ok(c) => c,
        Err(usage) => {
            eprintln!("controller-agent: {usage}");
            return boot_apply::EXIT_USAGE;
        }
    };
    let cfg = DeviceSettingsConfig::default();
    let store = DeviceSettingsJson::new(&cfg);
    let files = BootFilesFs::new(&cfg);
    match cmd {
        Command::Early => boot_apply::early(&store, &files),
        Command::FailsafeEarly(result) => boot_apply::failsafe_early(result.as_deref(), &files),
        Command::Late => boot_apply::late(&store, &DeviceCtlCli::new(&cfg)),
        Command::FailsafeLate => boot_apply::failsafe_late(&store, &DeviceCtlCli::new(&cfg)),
    }
}

fn main() {
    // `controller-agent provision` is the once-per-boot account setup run by
    // thing-revived-provision.service; it has its own logging and exit codes.
    if std::env::args().nth(1).as_deref() == Some("provision") {
        std::process::exit(provision::command());
    }
    // The device settings boot apply must not depend on controller.toml.
    if std::env::args().nth(1).as_deref() == Some("apply-device-settings") {
        let rest: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(apply_device_settings(&rest));
    }
    // The logger needs the config, so a config failure is reported straight
    // to stderr (journald captures it) before logging exists.
    let loaded = match Config::load() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("controller-agent: {e}");
            std::process::exit(EX_CONFIG);
        }
    };
    logging::init(&loaded.config.logging);
    controller_agent::run(loaded);
}
