//! uinput virtual keyboard used to selectively replay KEY_M and KEY_ENTER
//! once the real buttons device is grabbed exclusively.

use crate::app::error::AppError;
use crate::ports::KeyReplay;
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, Key};

pub struct UinputReplay {
    dev: VirtualDevice,
}

impl UinputReplay {
    /// Capability set is deliberately narrow: exactly the two keys that must
    /// keep behaving like ordinary keystrokes in the browser. Panics at
    /// startup if /dev/uinput is unusable, as before.
    pub fn build() -> UinputReplay {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(Key::KEY_M);
        keys.insert(Key::KEY_ENTER);
        let dev = VirtualDeviceBuilder::new()
            .expect("uinput: /dev/uinput open failed")
            .name("thing-revived-passthrough")
            .with_keys(&keys)
            .expect("uinput: with_keys failed")
            .build()
            .expect("uinput: virtual device build failed");
        UinputReplay { dev }
    }
}

impl KeyReplay for UinputReplay {
    fn replay(&mut self, code: u16, value: i32) -> Result<(), AppError> {
        self.dev.emit(&[InputEvent::new(EventType::KEY, code, value)]).map_err(AppError::failed)
    }
}
