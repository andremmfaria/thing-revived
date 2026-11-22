//! Raw Linux KEY_* codes seen on the buttons device (confirmed live) and the
//! rule for which of them are replayed to the browser.

pub const KEY_ESC: u16 = 1;
pub const KEY_1: u16 = 2;
pub const KEY_4: u16 = 5;
pub const KEY_ENTER: u16 = 28;
pub const KEY_M: u16 = 50;

/// Only M and Enter keep acting like ordinary keystrokes after the device is
/// grabbed; 1-4 and ESC exist only as the WebSocket `button` message.
pub fn is_replayed(code: u16) -> bool {
    code == KEY_M || code == KEY_ENTER
}

/// True for the six keys the hardware is documented to report. Anything else
/// is logged as unexpected but still broadcast.
pub fn is_known(code: u16) -> bool {
    (KEY_1..=KEY_4).contains(&code) || code == KEY_ESC || code == KEY_M || code == KEY_ENTER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_table_is_exactly_m_and_enter() {
        let replayed: Vec<u16> = (0..=255).filter(|c| is_replayed(*c)).collect();
        assert_eq!(replayed, [KEY_ENTER, KEY_M]);
    }

    #[test]
    fn known_keys_are_the_six_documented_ones() {
        let known: Vec<u16> = (0..=255).filter(|c| is_known(*c)).collect();
        assert_eq!(known, [KEY_ESC, 2, 3, 4, 5, KEY_ENTER, KEY_M]);
    }
}
