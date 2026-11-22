//! Incremental UTF-8 decoding for chunked PTY reads.
//!
//! A read can end in the middle of a multibyte character. Decoding each chunk
//! on its own with `from_utf8_lossy` turns both halves into U+FFFD. This keeps
//! the incomplete tail (at most 3 bytes) and prepends it to the next chunk.
//! Bytes that are genuinely invalid still become U+FFFD, same as lossy decode.

#[derive(Default)]
pub struct Utf8Carry {
    pending: Vec<u8>,
}

impl Utf8Carry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes `chunk` after any carried bytes. May return an empty string if
    /// the chunk only held part of a character.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        let joined;
        let mut rest: &[u8] = if self.pending.is_empty() {
            chunk
        } else {
            joined = [self.pending.as_slice(), chunk].concat();
            &joined
        };
        let mut out = String::with_capacity(rest.len());
        let mut tail: Vec<u8> = Vec::new();
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    break;
                }
                Err(e) => {
                    let (valid, after) = rest.split_at(e.valid_up_to());
                    // valid_up_to guarantees this prefix is valid UTF-8.
                    out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            rest = &after[n..];
                        }
                        // Incomplete sequence at the very end: keep for next push.
                        None => {
                            tail.extend_from_slice(after);
                            break;
                        }
                    }
                }
            }
        }
        self.pending = tail;
        out
    }

    /// Call at end of stream: a dangling incomplete sequence is invalid.
    pub fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{FFFD}".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_split(bytes: &[u8], at: usize) -> String {
        let mut c = Utf8Carry::new();
        let mut s = c.push(&bytes[..at]);
        s.push_str(&c.push(&bytes[at..]));
        s.push_str(&c.finish());
        s
    }

    #[test]
    fn split_at_every_position() {
        for text in ["a\u{e9}b", "x\u{20ac}y", "p\u{1F600}q", "\u{2500}\u{2502}\u{4e2d}\u{6587}", "\u{1F600}\u{1F680}"] {
            let bytes = text.as_bytes();
            for at in 0..=bytes.len() {
                assert_eq!(decode_split(bytes, at), text, "split {at} of {text:?}");
            }
        }
    }

    #[test]
    fn split_into_single_bytes() {
        let text = "h\u{e9}\u{20ac}\u{1F600}!";
        let mut c = Utf8Carry::new();
        let mut out = String::new();
        for b in text.as_bytes() {
            out.push_str(&c.push(&[*b]));
        }
        out.push_str(&c.finish());
        assert_eq!(out, text);
    }

    #[test]
    fn invalid_bytes_become_replacement() {
        let mut c = Utf8Carry::new();
        assert_eq!(c.push(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(c.push(b"\x80"), "\u{FFFD}");
        // Lead byte followed by a non-continuation byte.
        assert_eq!(c.push(b"\xe2\x28"), "\u{FFFD}(");
        assert_eq!(c.finish(), "");
    }

    #[test]
    fn matches_lossy_for_whole_chunks() {
        let cases: [&[u8]; 4] = [b"plain", b"a\xffb\xc0\xafc", "\u{20ac}".as_bytes(), b"\xf0\x9f\x98x"];
        for case in cases {
            let mut c = Utf8Carry::new();
            let mut got = c.push(case);
            got.push_str(&c.finish());
            assert_eq!(got, String::from_utf8_lossy(case));
        }
    }

    #[test]
    fn dangling_incomplete_at_end_is_replacement() {
        let mut c = Utf8Carry::new();
        assert_eq!(c.push(&[0xe2, 0x82]), "");
        assert_eq!(c.finish(), "\u{FFFD}");
        assert_eq!(c.finish(), "");
    }

    #[test]
    fn incomplete_then_invalid_continuation() {
        let mut c = Utf8Carry::new();
        assert_eq!(c.push(&[0xe2, 0x82]), "");
        assert_eq!(c.push(b"A"), "\u{FFFD}A");
    }
}
