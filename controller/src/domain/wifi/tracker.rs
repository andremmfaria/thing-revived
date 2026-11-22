//! Folds the events of one connect attempt into a verdict.

use super::events::WpaEvent;

/// Why a connect attempt failed, with the stable `error` code the frontend
/// switches on and the user-facing message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailKind {
    WrongPassword,
    NotFound,
    AssocRejected,
    AuthRejected,
    NoResponse,
    DhcpTimeout,
    Unavailable(String),
    InvalidInput(String),
}

impl FailKind {
    pub fn code(&self) -> &'static str {
        match self {
            FailKind::WrongPassword => "wrong_password",
            FailKind::NotFound => "network_not_found",
            FailKind::AssocRejected => "assoc_rejected",
            FailKind::AuthRejected => "auth_rejected",
            FailKind::NoResponse => "no_response",
            FailKind::DhcpTimeout => "dhcp_timeout",
            FailKind::Unavailable(_) => "unavailable",
            FailKind::InvalidInput(_) => "invalid_input",
        }
    }

    pub fn message(&self) -> String {
        match self {
            FailKind::WrongPassword => "Wrong password".into(),
            FailKind::NotFound => "Network not found (out of range?)".into(),
            FailKind::AssocRejected => "The access point rejected the connection".into(),
            FailKind::AuthRejected => "Authentication was rejected".into(),
            FailKind::NoResponse => "No response from the access point".into(),
            FailKind::DhcpTimeout => "Connected, but no IP address was assigned (DHCP timeout)".into(),
            FailKind::Unavailable(m) | FailKind::InvalidInput(m) => m.clone(),
        }
    }
}

/// Folds the event stream of one connect attempt into a verdict. Returns
/// `Some(Err)` as soon as a terminal failure is certain; `Connected` is
/// reported through `connected()` because success also needs an IP.
pub struct ConnectTracker {
    has_password: bool,
    not_found: u32,
    assoc_rejects: u32,
    connected: bool,
}

impl ConnectTracker {
    pub fn new(has_password: bool) -> Self {
        ConnectTracker { has_password, not_found: 0, assoc_rejects: 0, connected: false }
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn on_event(&mut self, ev: &WpaEvent) -> Option<FailKind> {
        match ev {
            WpaEvent::Connected => {
                self.connected = true;
                None
            }
            WpaEvent::HandshakeFailed if self.has_password && !self.connected => Some(FailKind::WrongPassword),
            WpaEvent::TempDisabled { reason } if !self.connected => match reason.as_str() {
                "WRONG_KEY" => Some(FailKind::WrongPassword),
                "AUTH_FAILED" | "CONN_FAILED" => {
                    Some(if self.has_password { FailKind::WrongPassword } else { FailKind::AuthRejected })
                }
                _ => None,
            },
            WpaEvent::NetworkNotFound if !self.connected => {
                self.not_found += 1;
                // The first miss can be a stale scan; the second is real.
                if self.not_found >= 2 {
                    Some(FailKind::NotFound)
                } else {
                    None
                }
            }
            WpaEvent::AssocReject if !self.connected => {
                self.assoc_rejects += 1;
                if self.assoc_rejects >= 3 {
                    Some(FailKind::AssocRejected)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Failure to report when the overall timeout elapses with no verdict.
    pub fn timeout_kind(&self) -> FailKind {
        if self.not_found > 0 {
            FailKind::NotFound
        } else {
            FailKind::NoResponse
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_reports_wrong_password() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::Other), None);
        assert_eq!(
            t.on_event(&WpaEvent::TempDisabled { reason: "WRONG_KEY".into() }),
            Some(FailKind::WrongPassword)
        );
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::HandshakeFailed), Some(FailKind::WrongPassword));
        let mut t = ConnectTracker::new(false);
        assert_eq!(
            t.on_event(&WpaEvent::TempDisabled { reason: "CONN_FAILED".into() }),
            Some(FailKind::AuthRejected)
        );
    }

    #[test]
    fn tracker_not_found_needs_two_misses_and_reject_needs_three() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::NetworkNotFound), None);
        assert_eq!(t.on_event(&WpaEvent::NetworkNotFound), Some(FailKind::NotFound));
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), None);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), None);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), Some(FailKind::AssocRejected));
    }

    #[test]
    fn tracker_connected_stops_failure_reports_and_timeout_kind() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.timeout_kind(), FailKind::NoResponse);
        t.on_event(&WpaEvent::NetworkNotFound);
        assert_eq!(t.timeout_kind(), FailKind::NotFound);
        t.on_event(&WpaEvent::Connected);
        assert!(t.connected());
        assert_eq!(t.on_event(&WpaEvent::TempDisabled { reason: "WRONG_KEY".into() }), None);
    }

    #[test]
    fn fail_kinds_have_stable_codes_and_no_password_text() {
        assert_eq!(FailKind::WrongPassword.code(), "wrong_password");
        assert_eq!(FailKind::DhcpTimeout.code(), "dhcp_timeout");
        assert_eq!(FailKind::NoResponse.code(), "no_response");
        assert!(FailKind::DhcpTimeout.message().contains("DHCP"));
    }

    // --- end-to-end against a fake wpa_supplicant control socket ----------
}
