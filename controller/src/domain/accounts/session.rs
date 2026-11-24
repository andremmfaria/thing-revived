//! Unlock sessions: one per connection, a fixed time to live from the moment
//! of the unlock, never shared. Activity does not extend it; a fresh password
//! entry on the same connection is a fresh unlock and restarts the clock.
//! Time is injected monotonic seconds. A disconnect, an explicit lock, or the
//! target account being removed or locked drops the session.

use super::ratelimit::ConnKey;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// No session on this connection, or its time to live ran out.
    NotUnlocked,
}

#[derive(Debug, Clone)]
struct Session {
    admin: String,
    unlocked_at: u64,
}

#[derive(Debug)]
pub struct Sessions {
    ttl_secs: u64,
    by_conn: HashMap<ConnKey, Session>,
}

impl Sessions {
    /// `ttl_secs` is `[users] session_ttl_secs` (1800).
    pub fn new(ttl_secs: u64) -> Sessions {
        Sessions { ttl_secs, by_conn: HashMap::new() }
    }

    fn live(&self, s: &Session, now: u64) -> bool {
        now.saturating_sub(s.unlocked_at) < self.ttl_secs
    }

    /// Replaces any session on this connection and restarts its clock.
    pub fn unlock(&mut self, conn: ConnKey, admin: &str, now: u64) {
        self.by_conn.insert(conn, Session { admin: admin.to_string(), unlocked_at: now });
    }

    /// The admin name for a mutating command. Never extends the session; an
    /// expired one is removed and reported as `NotUnlocked`, exactly at the
    /// TTL.
    pub fn require(&mut self, conn: ConnKey, now: u64) -> Result<&str, SessionError> {
        let live = self.by_conn.get(&conn).is_some_and(|s| self.live(s, now));
        if !live {
            self.by_conn.remove(&conn);
            return Err(SessionError::NotUnlocked);
        }
        Ok(&self.by_conn.get(&conn).expect("checked live").admin)
    }

    /// Read-only view: admin name and seconds left.
    pub fn peek(&self, conn: ConnKey, now: u64) -> Option<(&str, u64)> {
        let s = self.by_conn.get(&conn).filter(|s| self.live(s, now))?;
        Some((&s.admin, self.ttl_secs - now.saturating_sub(s.unlocked_at)))
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

    /// Drops sessions past their TTL (the same predicate as `require`) and
    /// returns their connections.
    pub fn sweep(&mut self, now: u64) -> Vec<ConnKey> {
        let ttl = self.ttl_secs;
        let gone: Vec<ConnKey> =
            self.by_conn.iter().filter(|(_, s)| now.saturating_sub(s.unlocked_at) >= ttl).map(|(c, _)| *c).collect();
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
    fn unlock_require_and_expiry_at_exactly_the_ttl() {
        let mut s = Sessions::new(1800);
        assert_eq!(s.require(1, 0), Err(SessionError::NotUnlocked));
        s.unlock(1, "alice", 0);
        assert_eq!(s.require(1, 1799), Ok("alice"));
        assert_eq!(s.require(1, 1800), Err(SessionError::NotUnlocked), "refused at exactly the TTL");
        assert_eq!(s.require(1, 1801), Err(SessionError::NotUnlocked), "expired session is gone for good");
    }

    #[test]
    fn activity_does_not_extend_the_session() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        assert_eq!(s.require(1, 100), Ok("alice"));
        assert_eq!(s.require(1, 299), Ok("alice"));
        assert_eq!(s.peek(1, 299), Some(("alice", 1)), "the clock kept running from the unlock");
        assert_eq!(s.require(1, 300), Err(SessionError::NotUnlocked));
    }

    #[test]
    fn sessions_are_bound_to_their_connection_with_independent_clocks() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        assert_eq!(s.require(2, 1), Err(SessionError::NotUnlocked));
        s.unlock(2, "alice", 200);
        assert_eq!(s.require(1, 299), Ok("alice"));
        assert_eq!(s.require(2, 300), Ok("alice"), "connection 2 started later");
        assert_eq!(s.require(1, 300), Err(SessionError::NotUnlocked));
        assert!(s.lock(2));
        assert!(!s.lock(2));
        assert_eq!(s.require(2, 301), Err(SessionError::NotUnlocked));
    }

    #[test]
    fn peek_reports_time_left_without_extending() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        assert_eq!(s.peek(1, 100), Some(("alice", 200)));
        assert_eq!(s.peek(1, 100), Some(("alice", 200)));
        assert_eq!(s.peek(1, 300), None);
    }

    #[test]
    fn drop_admin_and_sweep_use_the_unlock_time() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        s.unlock(2, "alice", 0);
        s.unlock(3, "kiosk", 250);
        let mut gone = s.drop_admin("alice");
        gone.sort();
        assert_eq!(gone, vec![1, 2]);
        assert_eq!(s.sweep(549), Vec::<ConnKey>::new());
        assert_eq!(s.sweep(550), vec![3]);
    }

    #[test]
    fn sweep_and_require_agree_on_the_boundary() {
        let mut s = Sessions::new(60);
        s.unlock(1, "alice", 10);
        assert_eq!(s.sweep(69), Vec::<ConnKey>::new());
        assert_eq!(s.sweep(70), vec![1]);
        assert_eq!(s.require(1, 70), Err(SessionError::NotUnlocked));
    }

    #[test]
    fn a_fresh_unlock_on_the_same_connection_restarts_the_clock() {
        let mut s = Sessions::new(300);
        s.unlock(1, "alice", 0);
        s.unlock(1, "kiosk", 250);
        assert_eq!(s.require(1, 300), Ok("kiosk"));
        assert_eq!(s.require(1, 549), Ok("kiosk"));
        assert_eq!(s.require(1, 550), Err(SessionError::NotUnlocked));
    }
}
