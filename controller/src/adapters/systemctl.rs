//! `Restarter` over `systemctl restart <unit>`. Spawned, not awaited: the
//! KEY_M long press must never block on the unit coming back up.

use crate::app::error::AppError;
use crate::ports::Restarter;
use std::process::Command;

pub struct SystemctlRestarter {
    program: String,
}

impl SystemctlRestarter {
    pub fn new() -> SystemctlRestarter {
        SystemctlRestarter::with_program("systemctl")
    }

    /// Uses another program in place of `systemctl` (tests).
    pub fn with_program(program: &str) -> SystemctlRestarter {
        SystemctlRestarter { program: program.to_string() }
    }
}

impl Default for SystemctlRestarter {
    fn default() -> Self {
        SystemctlRestarter::new()
    }
}

impl Restarter for SystemctlRestarter {
    fn restart(&self, unit: &str) -> Result<(), AppError> {
        Command::new(&self.program)
            .args(["restart", unit])
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::failed(format!("failed to spawn {} restart {unit}: {e}", self.program)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failure_is_a_failed_error_naming_the_unit() {
        let e = SystemctlRestarter::with_program("/nonexistent/systemctl").restart("x.service").unwrap_err();
        assert!(e.message.contains("x.service"), "{e}");
    }

    #[test]
    fn a_spawnable_program_is_ok() {
        assert!(SystemctlRestarter::with_program("true").restart("x.service").is_ok());
    }
}
