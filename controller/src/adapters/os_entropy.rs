//! `Entropy` over the kernel's random device.
//!
//! `/dev/random` is read on purpose: since Linux 5.6 it blocks only until the
//! kernel's pool is initialised (never afterwards), which is exactly the
//! guarantee `getrandom(2)` gives, without an FFI call. `/dev/urandom` would
//! hand out bytes before that point.

use crate::app::error::AppError;
use crate::ports::Entropy;
use std::io::Read;
use std::path::PathBuf;

pub struct OsEntropy {
    device: PathBuf,
}

impl OsEntropy {
    pub fn new() -> OsEntropy {
        OsEntropy::with_device("/dev/random")
    }

    /// Reads from another file (tests).
    pub fn with_device(path: impl Into<PathBuf>) -> OsEntropy {
        OsEntropy { device: path.into() }
    }
}

impl Default for OsEntropy {
    fn default() -> Self {
        OsEntropy::new()
    }
}

impl Entropy for OsEntropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), AppError> {
        let mut f = std::fs::File::open(&self.device).map_err(|e| AppError::failed(format!("open {}: {e}", self.device.display())))?;
        f.read_exact(buf).map_err(|e| AppError::failed(format!("read {}: {e}", self.device.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_device_fills_and_two_draws_differ() {
        let e = OsEntropy::new();
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        e.fill(&mut a).unwrap();
        e.fill(&mut b).unwrap();
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn a_missing_device_is_an_error_naming_the_path() {
        let e = OsEntropy::with_device("/nonexistent/random");
        let err = e.fill(&mut [0u8; 4]).unwrap_err();
        assert!(err.message.contains("/nonexistent/random"), "{err}");
    }

    #[test]
    fn a_short_device_is_an_error_not_a_partial_fill() {
        let dir = std::env::temp_dir().join(format!("controller-entropy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let err = OsEntropy::with_device(&path).fill(&mut [0u8; 8]).unwrap_err();
        assert!(err.message.contains("read"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
