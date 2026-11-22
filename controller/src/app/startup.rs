//! Startup work that runs once before clients connect.

use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;

/// Re-applies the persisted brightness and timezone to the hardware, so a
/// reboot does not lose the user's settings. Missing values are skipped.
pub fn apply_persisted(display: &DisplayService, timezone: &TimezoneService) {
    display.apply_saved();
    timezone.apply_saved();
}
