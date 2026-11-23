//! Password type, validator and generator. Pure: randomness arrives as a
//! closure so tests can script it and the entropy adapter stays outside.

use std::fmt;

/// 57 symbols with the look-alikes `I O l 0 1` removed (about 93 bits for
/// 16 characters). The count is derived from the string, never hard-coded.
pub const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
/// Length of a generated password.
pub const GENERATED_LEN: usize = 16;
/// Accepted length range for a typed or reset password.
pub const MIN_LEN: usize = 12;
pub const MAX_LEN: usize = 128;
/// Bytes requested from the entropy source per draw.
const DRAW_BYTES: usize = 32;
/// Upper bound on draws, so a stuck source cannot loop forever.
const MAX_DRAWS: usize = 64;

/// A password that must never reach a log line: `Debug` and `Display` are
/// fixed text, and the bytes are overwritten on drop. This is the domain's
/// own type because `domain` may not depend on `protocol`.
#[derive(Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The raw value. Call sites must never pass it to a log macro.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Clone for Secret {
    fn clone(&self) -> Secret {
        Secret(self.0.clone())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|b| *b = 0);
        // Keeps the zeroing from being optimised away as a dead store.
        std::hint::black_box(&bytes);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    TooShort,
    TooLong,
    /// Anything outside printable ASCII (this also rules out newlines).
    BadCharacter,
    SameAsName,
    /// Only one character class (lower, upper, digit, other) is used.
    SingleClass,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PasswordError::TooShort => "password is shorter than 12 characters",
            PasswordError::TooLong => "password is longer than 128 characters",
            PasswordError::BadCharacter => "password has a character outside printable ASCII",
            PasswordError::SameAsName => "password equals the account name",
            PasswordError::SingleClass => "password uses only one kind of character",
        })
    }
}

impl std::error::Error for PasswordError {}

fn class_of(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => 0,
        b'A'..=b'Z' => 1,
        b'0'..=b'9' => 2,
        _ => 3,
    }
}

fn uses_two_classes(s: &str) -> bool {
    let mut seen = [false; 4];
    s.bytes().for_each(|b| seen[class_of(b) as usize] = true);
    seen.iter().filter(|x| **x).count() >= 2
}

/// Rules for a new or reset password: 12 to 128 printable ASCII characters,
/// not equal to the account name, not a single character class.
pub fn validate(name: &str, password: &str) -> Result<(), PasswordError> {
    if !password.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(PasswordError::BadCharacter);
    }
    if password.len() < MIN_LEN {
        return Err(PasswordError::TooShort);
    }
    if password.len() > MAX_LEN {
        return Err(PasswordError::TooLong);
    }
    if password == name {
        return Err(PasswordError::SameAsName);
    }
    if !uses_two_classes(password) {
        return Err(PasswordError::SingleClass);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum GenerateError<E> {
    /// The entropy source failed.
    Entropy(E),
    /// The source kept returning unusable bytes (a stuck or scripted source).
    Exhausted,
}

/// Draws a `GENERATED_LEN` character password. Rejection sampling: a byte is
/// used only below the largest multiple of the alphabet size that fits in
/// 256, so every symbol is equally likely (no modulo bias). A candidate that
/// would fail `validate` (one character class) is dropped and redrawn.
pub fn generate<E>(mut fill: impl FnMut(&mut [u8]) -> Result<(), E>) -> Result<Secret, GenerateError<E>> {
    let n = ALPHABET.len();
    let limit = 256 - (256 % n);
    let mut out: Vec<u8> = Vec::with_capacity(GENERATED_LEN);
    let mut buf = [0u8; DRAW_BYTES];
    for _ in 0..MAX_DRAWS {
        fill(&mut buf).map_err(GenerateError::Entropy)?;
        for &b in buf.iter() {
            if (b as usize) < limit {
                out.push(ALPHABET[b as usize % n]);
                if out.len() == GENERATED_LEN {
                    let candidate = String::from_utf8(std::mem::take(&mut out)).unwrap_or_default();
                    if uses_two_classes(&candidate) {
                        buf.iter_mut().for_each(|x| *x = 0);
                        return Ok(Secret::new(candidate));
                    }
                    // `candidate` is dropped here: a rejected password is
                    // a password nobody sees.
                    drop(Secret::new(candidate));
                }
            }
        }
    }
    buf.iter_mut().for_each(|x| *x = 0);
    Err(GenerateError::Exhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Hands out scripted bytes, then fails.
    struct Script(VecDeque<u8>);

    impl Script {
        fn new(bytes: &[u8]) -> Script {
            Script(bytes.iter().copied().collect())
        }

        fn fill(&mut self, buf: &mut [u8]) -> Result<(), &'static str> {
            for slot in buf.iter_mut() {
                *slot = self.0.pop_front().ok_or("script empty")?;
            }
            Ok(())
        }
    }

    #[test]
    fn alphabet_has_no_lookalikes_and_no_duplicates() {
        for c in b"IOl01" {
            assert!(!ALPHABET.contains(c), "{}", *c as char);
        }
        let mut sorted = ALPHABET.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ALPHABET.len());
    }

    #[test]
    fn maps_bytes_to_symbols_and_rejects_the_biased_tail() {
        let n = ALPHABET.len();
        let limit = (256 - 256 % n) as u8;
        // 16 good bytes in the middle of the range, with rejected tail bytes
        // sprinkled in: they must be skipped, not wrapped.
        let mut bytes = Vec::new();
        for i in 0..16u8 {
            bytes.push(255);
            bytes.push(limit);
            bytes.push(i * 3);
        }
        bytes.resize(DRAW_BYTES * 2, 0);
        let mut script = Script::new(&bytes);
        let pw = generate(|b| script.fill(b)).unwrap();
        let want: String = (0..16u8).map(|i| ALPHABET[(i * 3) as usize % n] as char).collect();
        assert_eq!(pw.expose(), want);
        assert_eq!(pw.len(), GENERATED_LEN);
    }

    #[test]
    fn every_symbol_is_equally_likely_over_the_full_byte_range() {
        // Feeding each byte value 0..=255 exactly once is a complete census
        // of the source: a modulo reduction would give some symbols a second
        // slot, rejection sampling gives each exactly `limit / n`.
        let n = ALPHABET.len();
        let mut counts = vec![0usize; n];
        let limit = 256 - 256 % n;
        for b in 0..=255usize {
            if b < limit {
                counts[b % n] += 1;
            }
        }
        let first = counts[0];
        assert!(counts.iter().all(|c| *c == first), "{counts:?}");
        assert_eq!(first, limit / n);
        // And the generator really consumes bytes this way: count symbols
        // produced from a long run of a simple deterministic generator.
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut seen = vec![0usize; n];
        for _ in 0..4000 {
            let pw = generate(|buf: &mut [u8]| -> Result<(), ()> {
                for slot in buf.iter_mut() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *slot = (state >> 24) as u8;
                }
                Ok(())
            })
            .unwrap();
            for c in pw.expose().bytes() {
                seen[ALPHABET.iter().position(|a| *a == c).unwrap()] += 1;
            }
        }
        let total: usize = seen.iter().sum();
        let expected = total as f64 / n as f64;
        for (i, c) in seen.iter().enumerate() {
            let dev = (*c as f64 - expected).abs() / expected;
            assert!(dev < 0.08, "symbol {i} deviates {dev:.3}: {c} vs {expected:.0}");
        }
    }

    #[test]
    fn a_single_class_candidate_is_redrawn() {
        // 16 bytes mapping to 'A' (index 0), then 16 mixed.
        let mut bytes = vec![0u8; 16];
        bytes.extend((0..16u8).map(|i| i * 3));
        bytes.resize(DRAW_BYTES * 2, 0);
        let mut script = Script::new(&bytes);
        let pw = generate(|b| script.fill(b)).unwrap();
        assert_ne!(pw.expose(), "AAAAAAAAAAAAAAAA");
        assert!(validate("kiosk", pw.expose()).is_ok());
    }

    #[test]
    fn entropy_failure_is_passed_through() {
        let mut script = Script::new(&[]);
        assert_eq!(generate(|b| script.fill(b)).err(), Some(GenerateError::Entropy("script empty")));
    }

    #[test]
    fn a_stuck_source_gives_up() {
        let r = generate(|b: &mut [u8]| -> Result<(), ()> {
            b.fill(255);
            Ok(())
        });
        assert_eq!(r.err(), Some(GenerateError::Exhausted));
    }

    #[test]
    fn generated_passwords_pass_the_validator() {
        let mut state = 7u64;
        for _ in 0..200 {
            let pw = generate(|buf: &mut [u8]| -> Result<(), ()> {
                for slot in buf.iter_mut() {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    *slot = (state >> 33) as u8;
                }
                Ok(())
            })
            .unwrap();
            assert!(validate("kiosk", pw.expose()).is_ok());
            assert!(pw.expose().bytes().all(|b| ALPHABET.contains(&b)));
        }
    }

    #[test]
    fn validator_rules() {
        assert_eq!(validate("kiosk", "Abcdefgh123"), Err(PasswordError::TooShort));
        assert!(validate("kiosk", "Abcdefgh1234").is_ok());
        assert_eq!(validate("kiosk", &"aB1".repeat(50)), Err(PasswordError::TooLong));
        assert_eq!(validate("kiosk", "aaaaaaaaaaaaaa"), Err(PasswordError::SingleClass));
        assert_eq!(validate("kiosk", "123456789012345"), Err(PasswordError::SingleClass));
        assert_eq!(validate("kiosk", "abcdefgh\n12345"), Err(PasswordError::BadCharacter));
        assert_eq!(validate("kiosk", "abcdefgh\u{e9}12345"), Err(PasswordError::BadCharacter));
        assert_eq!(validate("long.user.name", "long.user.name"), Err(PasswordError::SameAsName));
        assert!(validate("kiosk", "with spaces and 1").is_ok());
    }

    #[test]
    fn secret_is_redacted_everywhere_it_is_formatted() {
        let s = Secret::new("hunter2hunter2");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(format!("{:#?}", Some(&s)), "Some(\n    <redacted>,\n)");
        assert!(!format!("{s:?}{s}").contains("hunter"));
        assert_eq!(s.expose(), "hunter2hunter2");
    }

}
