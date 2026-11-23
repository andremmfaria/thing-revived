//! Authorized-key line parser (Phase 2b). One line, no options, a known key
//! type whose base64 blob names the same type, at most 8 KiB. Fingerprints
//! come from `ssh-keygen` behind the `KeyInspector` port; this module only
//! decides what may be written to a key file.
//!
//! Keys are optional for every user including `kiosk`: nothing is baked and
//! a user with no key file simply logs in with a password (delta section 9).

use std::fmt;

pub const MAX_KEY_BYTES: usize = 8 * 1024;
pub const MAX_COMMENT_LEN: usize = 128;

/// sshd accepts these in `authorized_keys`; anything else is refused.
pub const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    TooLong,
    /// More than one line, or a control character such as NUL.
    MultiLine,
    /// First word is not a known key type: this also rejects every option
    /// prefix (`command=`, `from=`, `no-pty`, ...).
    UnknownType,
    /// Blob missing, not base64, or its embedded type differs.
    BadBlob,
    BadComment,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyError::Empty => "no key given",
            KeyError::TooLong => "key is longer than 8 KiB",
            KeyError::MultiLine => "paste exactly one key on one line",
            KeyError::UnknownType => "unknown key type (options are not accepted)",
            KeyError::BadBlob => "the key data is not valid",
            KeyError::BadComment => "the comment is too long or has control characters",
        })
    }
}

impl std::error::Error for KeyError {}

/// A key that passed the parser, in the form to append to the key file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKey {
    pub key_type: String,
    /// Base64 blob, exactly as given.
    pub blob: String,
    pub comment: String,
}

impl ParsedKey {
    /// `type blob[ comment]`, no trailing newline.
    pub fn line(&self) -> String {
        if self.comment.is_empty() {
            format!("{} {}", self.key_type, self.blob)
        } else {
            format!("{} {} {}", self.key_type, self.blob, self.comment)
        }
    }
}

fn b64_val(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Standard padded base64; strict enough for an SSH key blob.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let pad = bytes.iter().rev().take_while(|b| **b == b'=').count();
    if pad > 2 {
        return None;
    }
    let body = &bytes[..bytes.len() - pad];
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in body {
        acc = (acc << 6) | b64_val(b)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The first field of an SSH wire blob is a length-prefixed type string.
fn blob_type(blob: &[u8]) -> Option<&str> {
    let len = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    std::str::from_utf8(blob.get(4..4usize.checked_add(len)?)?).ok()
}

pub fn parse_line(input: &str) -> Result<ParsedKey, KeyError> {
    // One trailing newline is what a paste usually carries; strip only that.
    let text = input.strip_suffix("\r\n").or_else(|| input.strip_suffix('\n')).unwrap_or(input);
    if text.trim().is_empty() {
        return Err(KeyError::Empty);
    }
    if text.len() > MAX_KEY_BYTES {
        return Err(KeyError::TooLong);
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(KeyError::MultiLine);
    }
    let mut parts = text.trim().splitn(3, ' ');
    let key_type = parts.next().unwrap_or("");
    if !KEY_TYPES.contains(&key_type) {
        return Err(KeyError::UnknownType);
    }
    let blob = parts.next().unwrap_or("");
    let raw = b64_decode(blob).ok_or(KeyError::BadBlob)?;
    if blob_type(&raw) != Some(key_type) {
        return Err(KeyError::BadBlob);
    }
    let comment = parts.next().unwrap_or("").trim();
    if comment.len() > MAX_COMMENT_LEN {
        return Err(KeyError::BadComment);
    }
    Ok(ParsedKey { key_type: key_type.to_string(), blob: blob.to_string(), comment: comment.to_string() })
}

/// Count limit (`max_keys_per_user`): `existing` is the current key count.
pub fn check_key_count(existing: usize, max: usize) -> bool {
    existing < max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    fn blob_for(t: &str) -> String {
        let mut raw = (t.len() as u32).to_be_bytes().to_vec();
        raw.extend_from_slice(t.as_bytes());
        raw.extend_from_slice(&[1, 2, 3, 4, 5]);
        b64(&raw)
    }

    fn line(t: &str, comment: &str) -> String {
        format!("{t} {} {comment}", blob_for(t)).trim_end().to_string()
    }

    #[test]
    fn accepts_a_plain_key_and_normalises() {
        let l = line("ssh-ed25519", "me@host laptop");
        let k = parse_line(&format!("{l}\n")).unwrap();
        assert_eq!(k.key_type, "ssh-ed25519");
        assert_eq!(k.comment, "me@host laptop");
        assert_eq!(k.line(), l);
        assert_eq!(parse_line(&line("ssh-rsa", "")).unwrap().comment, "");
    }

    #[test]
    fn accepts_every_listed_type() {
        for t in KEY_TYPES {
            assert!(parse_line(&line(t, "c")).is_ok(), "{t}");
        }
    }

    #[test]
    fn rejects_options_and_unknown_types() {
        let l = line("ssh-ed25519", "c");
        for bad in [
            format!("command=\"id\" {l}"),
            format!("no-pty {l}"),
            format!("from=\"1.2.3.4\",{l}"),
            line("ssh-dss", "c"),
            "garbage".to_string(),
        ] {
            assert_eq!(parse_line(&bad), Err(KeyError::UnknownType), "{bad}");
        }
    }

    #[test]
    fn rejects_multiple_lines_and_control_chars() {
        let l = line("ssh-ed25519", "c");
        assert_eq!(parse_line(&format!("{l}\n{l}")), Err(KeyError::MultiLine));
        assert_eq!(parse_line(&format!("{l}\r{l}")), Err(KeyError::MultiLine));
        assert_eq!(parse_line(&format!("{l}\0")), Err(KeyError::MultiLine));
        assert_eq!(parse_line(&format!("{l}\t x")), Err(KeyError::MultiLine));
    }

    #[test]
    fn rejects_empty_and_oversized() {
        assert_eq!(parse_line(""), Err(KeyError::Empty));
        assert_eq!(parse_line("  \n"), Err(KeyError::Empty));
        let big = format!("ssh-ed25519 {}", "A".repeat(MAX_KEY_BYTES));
        assert_eq!(parse_line(&big), Err(KeyError::TooLong));
    }

    #[test]
    fn rejects_bad_blobs() {
        assert_eq!(parse_line("ssh-ed25519"), Err(KeyError::BadBlob));
        assert_eq!(parse_line("ssh-ed25519 !!!!"), Err(KeyError::BadBlob));
        assert_eq!(parse_line("ssh-ed25519 AAA"), Err(KeyError::BadBlob), "length not a multiple of 4");
        // valid base64 whose embedded type is another type
        let wrong = format!("ssh-ed25519 {}", blob_for("ssh-rsa"));
        assert_eq!(parse_line(&wrong), Err(KeyError::BadBlob));
        // truncated blob (declared length longer than data)
        let trunc = format!("ssh-ed25519 {}", b64(&[0, 0, 0, 99, 1]));
        assert_eq!(parse_line(&trunc), Err(KeyError::BadBlob));
    }

    #[test]
    fn rejects_long_comment() {
        let l = line("ssh-ed25519", &"x".repeat(MAX_COMMENT_LEN + 1));
        assert_eq!(parse_line(&l), Err(KeyError::BadComment));
    }

    #[test]
    fn base64_roundtrip_helper_matches_decoder() {
        for n in 0..8usize {
            let data: Vec<u8> = (0..n as u8 + 1).collect();
            assert_eq!(b64_decode(&b64(&data)).unwrap(), data);
        }
    }

    #[test]
    fn key_count_limit() {
        assert!(check_key_count(19, 20));
        assert!(!check_key_count(20, 20));
    }
}
