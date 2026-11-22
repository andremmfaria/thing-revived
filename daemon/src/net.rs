//! Listen address resolution and WebSocket Origin policy.
//!
//! The agent is an unauthenticated shell, so the only real access control is
//! where it listens. The default is loopback; anything wider must be asked for
//! explicitly through `TERMINAL_AGENT_BIND` or `[server] bind` in the config.

use std::net::SocketAddr;

pub const BIND_ENV: &str = "TERMINAL_AGENT_BIND";
pub use crate::utils::config::DEFAULT_BIND;

/// Resolves the listen address: a non-blank env value wins over the
/// configured one (`[server] bind`, default loopback). An unparsable value is
/// an error rather than a silent fallback, so a typo can never widen (or
/// quietly change) the bind.
pub fn resolve_bind(env_value: Option<&str>, configured: &str) -> Result<SocketAddr, String> {
    let value = match env_value.map(str::trim) {
        Some(v) if !v.is_empty() => v,
        _ => configured.trim(),
    };
    value
        .parse::<SocketAddr>()
        .map_err(|e| format!("{BIND_ENV}={value:?} is not a valid ip:port address: {e}"))
}

/// Whether Origin policy for the handshake allows this `Origin` header value.
///
/// Accepted: header absent (non-browser clients), the literal `null`
/// (Chromium sends it for `file://` pages, which is how the kiosk frontend is
/// loaded), and `http://localhost` or `http://127.0.0.1` with an optional
/// numeric port. The host is compared exactly, so `http://localhost.evil.com`
/// is rejected. Everything else, including https and other hosts, is rejected.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    let origin = match origin {
        None => return true,
        Some(o) => o.trim().to_ascii_lowercase(),
    };
    if origin == "null" {
        return true;
    }
    let rest = match origin.strip_prefix("http://") {
        Some(r) => r,
        None => return false,
    };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if host != "localhost" && host != "127.0.0.1" {
        return false;
    }
    match port {
        None => true,
        Some(p) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_is_loopback() {
        let addr = resolve_bind(None, DEFAULT_BIND).unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 8766);
        assert_eq!(addr.to_string(), DEFAULT_BIND);
        assert!(DEFAULT_BIND.parse::<SocketAddr>().unwrap().ip().is_loopback());
    }

    #[test]
    fn blank_bind_uses_default() {
        assert_eq!(resolve_bind(Some("  "), DEFAULT_BIND).unwrap().to_string(), DEFAULT_BIND);
    }

    #[test]
    fn explicit_bind_is_honoured() {
        let a = resolve_bind(Some("0.0.0.0:9000"), DEFAULT_BIND).unwrap();
        assert!(!a.ip().is_loopback());
        assert_eq!(a.port(), 9000);
    }

    #[test]
    fn bad_bind_is_an_error_not_a_fallback() {
        assert!(resolve_bind(Some("nonsense"), DEFAULT_BIND).is_err());
        assert!(resolve_bind(Some("127.0.0.1"), DEFAULT_BIND).is_err());
        assert!(resolve_bind(None, "nonsense").is_err());
    }

    #[test]
    fn env_beats_file_and_file_beats_default() {
        assert_eq!(resolve_bind(Some("127.0.0.1:1"), "127.0.0.1:2").unwrap().port(), 1);
        assert_eq!(resolve_bind(None, "127.0.0.1:2").unwrap().port(), 2);
        assert_eq!(resolve_bind(Some(""), "127.0.0.1:2").unwrap().port(), 2);
    }

    #[test]
    fn origin_policy() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("null")));
        assert!(origin_allowed(Some("http://localhost")));
        assert!(origin_allowed(Some("http://localhost:8080")));
        assert!(origin_allowed(Some("http://127.0.0.1:8766")));
        assert!(origin_allowed(Some("HTTP://LocalHost:1")));
        assert!(!origin_allowed(Some("http://localhost.evil.com")));
        assert!(!origin_allowed(Some("http://127.0.0.1.evil.com")));
        assert!(!origin_allowed(Some("http://localhost:")));
        assert!(!origin_allowed(Some("http://localhost:80a")));
        assert!(!origin_allowed(Some("http://localhost/x")));
        assert!(!origin_allowed(Some("https://localhost")));
        assert!(!origin_allowed(Some("http://example.com")));
        assert!(!origin_allowed(Some("http://192.168.1.5:8766")));
        assert!(!origin_allowed(Some("")));
    }
}
