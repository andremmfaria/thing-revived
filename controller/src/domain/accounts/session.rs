//! Unlock sessions: one per connection, idle-expiring, never shared. Time is
//! injected monotonic seconds. A disconnect, an explicit lock, or the target
//! account being removed or locked drops the session.

use super::ratelimit::ConnKey;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// No session on this connection, or it idled out.
    NotUnlocked,
}

#[derive(Debug, Clone)]
struct Session {
    admin: String,
    last_active: u64,
}

#[derive(Debug)]
pub struct Sessions {
    idle_secs: u64,
    by_conn: HashMap<ConnKey, Session>,
}

impl Sessions {
    /// `idle_secs` is `[users] session_idle_secs` (300).
    pub fn new(idle_secs: u64) -> Sessions {
        Sessions { idle_secs, by_conn: HashMap::new() }
    }

    fn live(&self, s: &Session, now: u64) -> bool {
        now.saturating_sub(s.last_active) < self.idle_secs
    }

    /// Replaces any session on this connection.
    pub fn unlock(&mut self, conn: ConnKey, admin: &str, now: u64) {
        self.by_conn.insert(conn, Session { admin: admin.to_string(), last_active: now });
    }

    /// The admin name for a mutating command. Refreshes the idle timer;
    /// an expired session is removed and reported as `NotUnlocked`.
    pub fn require(&mut self, conn: ConnKey, now: u64) -> Result<&str, SessionError> {
        let live = self.by_conn.get(&conn).is_some_and(|s| self.live(s, now));
        if !live {
            self.by_conn.remove(&conn);
            return Err(SessionError::NotUnlocked);
        }
        let s = self.by_conn.get_mut(&conn).expect("checked live");
        s.last_active = now;
        Ok(&s.admin)
    }

    /// Read-only view for `user_session`: admin name and seconds left.
    pub fn peek(&self, conn: ConnKey, now: u64) -> Option<(&str, u64)> {
        let s = self.by_conn.get(&conn).filter(|s| self.live(s, now))?;
        Some((&s.admin, self.idle_secs - now.saturating_sub(s.last_active)))
    }

    /// `user_lock_session` and disconnect.
    pub fn lock(&mut self, conn: ConnKey) -> bool {
        self.by_conn.remove(&conn).is_some()
    }

    /// The admin account was removed, locked or demoted: every session it
    /// owns ends. Returns the affected connections so the caller can notify.
    pub fn drop_admin(&mut self, admin: &str) -> Vec<ConnKey> {
        let gone: Vec<ConnKey> = self.by_conn.iter().filter(|(_, s)| s.admin == admin).map(|(c, _)| *c).collect();
        for c in &gone {
            self.by_conn.remove(c);
        }
        gone
    }

    /// Drops idled-out sessions and returns their connections.
    pub fn sweep(&mut self, now: u64) -> Vec<ConnKey> {
        let idle = self.idle_secs;
        let gone: Vec<ConnKey> =
            self.by_conn.iter().filter(|(_, s)| now.saturating_sub(s.last_active) >= idle).map(|(c, _)| *c).collect();
        for c in &gone {
            self.by_conn.remove(c);
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_require_and_idle_expiry() {
        let mut s = Sessions::new(300);
        assert_eq!(s.require(1, 0), Err(SessionError::NotUnlocked));
        s.unlock(1, "alice", 0);
        assert_eq!(s.require(1, 299), Ok("alice"));
        assert_eq!(s.require(1, 598), Ok("alice"), "activity refreshes the timer");
        assert_eq!(s.require(1, 898), Err(SessionError::NotUnlocked));
        assert_eq!(s.require(1, 899), Err(SessionError::NotUnlocked), "expired session is gone for good");
    }

    #[test]
    fn sessions_are_bound_to_their_connection() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        assert_eq!(s.require(2, 1), Err(SessionError::NotUnlocked));
        assert!(s.lock(1));
        assert!(!s.lock(1));
        assert_eq!(s.require(1, 1), Err(SessionError::NotUnlocked));
    }

    #[test]
    fn peek_reports_time_left_without_refreshing() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        assert_eq!(s.peek(1, 100), Some(("alice", 200)));
        assert_eq!(s.peek(1, 100), Some(("alice", 200)));
        assert_eq!(s.peek(1, 300), None);
    }

    #[test]
    fn drop_admin_and_sweep() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        s.unlock(2, "alice", 0);
        s.unlock(3, "kiosk", 250);
        let mut gone = s.drop_admin("alice");
        gone.sort();
        assert_eq!(gone, vec![1, 2]);
        assert_eq!(s.sweep(299), Vec::<ConnKey>::new());
        assert_eq!(s.sweep(550), vec![3]);
    }

    #[test]
    fn relock_replaces_the_session() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        s.unlock(1, "kiosk", 10);
        assert_eq!(s.require(1, 11), Ok("kiosk"));
    }
}
