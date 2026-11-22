use controller_agent::utils::config::{Config, EX_CONFIG};
use controller_agent::utils::logging;

fn main() {
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
