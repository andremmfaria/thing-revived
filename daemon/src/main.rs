//! terminal-agent binary: load config, set up logging, run the relay.

use terminal_agent::utils::config::{self, LoggingConfig, EXIT_CONFIG};
use terminal_agent::utils::logging;

fn main() {
    let loaded = match config::load() {
        Ok(l) => l,
        Err(e) => {
            // No config yet, so log with defaults; stderr reaches journald.
            logging::init(&LoggingConfig::default());
            log::error!("{e}");
            std::process::exit(EXIT_CONFIG);
        }
    };
    logging::init(&loaded.config.logging);
    if loaded.from_file {
        log::info!("config loaded from {}", loaded.path.display());
    } else {
        log::warn!("config file {} not found, using built-in defaults", loaded.path.display());
    }
    if let Err(e) = terminal_agent::run(loaded.config) {
        log::error!("{}", e.message);
        std::process::exit(e.exit_code);
    }
}
