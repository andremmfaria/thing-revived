mod provision;

use controller_agent::utils::config::{Config, EX_CONFIG};
use controller_agent::utils::logging;

fn main() {
    // `controller-agent provision` is the once-per-boot account setup run by
    // thing-revived-provision.service; it has its own logging and exit codes.
    if std::env::args().nth(1).as_deref() == Some("provision") {
        std::process::exit(provision::command());
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
