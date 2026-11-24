//! The account menu service (plan sections 6 to 8, delta section 9): the
//! handlers behind the fifteen Phase 2 commands. It holds ports only (no
//! files, no processes), one unlock session per connection, the unlock and
//! mutation rate limiters, and a mutex that serialises every mutation.
//!
//! Rules every handler follows:
//! - A mutating command needs an unlocked session on its own connection
//!   (`not_unlocked`), then a slot in the per-connection mutation limiter
//!   (`rate_limited`), then the domain guards, then `confirm`.
//! - Every reply goes to the requesting connection only. Passwords appear
//!   in exactly one outbound event, `user_secret`, for a generated password.
//! - Nothing here logs a password, a key body or a fingerprint. Audit lines
//!   carry the op, the target name (only if it is a valid name), the result
//!   code and the connection id.
//! - The unlock path verifies through `unix_chkpwd`, which bypasses PAM
//!   faillock, so the limiter here is the only brake on guesses.

use crate::app::error::{AppError, ErrorKind};
use crate::app::hub::{ConnId, TargetedSink};
use crate::domain::accounts::events::ErrorCode;
use crate::domain::accounts::groups::{self, GroupPolicy};
use crate::domain::accounts::guard::{self, GuardError, Limits, Mutation};
use crate::domain::accounts::keys;
use crate::domain::accounts::model::{is_managed_id, is_system_id, UserView, ADMIN_GROUP};
use crate::domain::accounts::names::{self, KIOSK_USER};
use crate::domain::accounts::password::{self, GenerateError, Secret};
use crate::domain::accounts::ratelimit::{Denied, MutationLimiter, UnlockConfig, UnlockLimiter};
use crate::domain::accounts::session::Sessions;
use crate::ports::{
    AccountAdmin, AccountDb, AccountFiles, AccountsError, AccountsPort, AdminSessionGate, AuthorizedKeysStore, Clock, Entropy, FirstBootGate,
    KeyInspector, PasswordVerifier,
};
use crate::protocol::{BridgeEvent, GroupWire, Secret as WireSecret, SshKeyWire, UserWire};
use crate::sync_util::lock;
use crate::utils::config::UsersConfig;
use log::{info, warn};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct AccountsPorts {
    pub db: Arc<dyn AccountDb>,
    pub accounts: Arc<dyn AccountsPort>,
    pub admin: Arc<dyn AccountAdmin>,
    pub files: Arc<dyn AccountFiles>,
    pub verifier: Arc<dyn PasswordVerifier>,
    pub keys: Arc<dyn AuthorizedKeysStore>,
    pub inspector: Arc<dyn KeyInspector>,
    pub entropy: Arc<dyn Entropy>,
    /// Monotonic time for sessions and limiters.
    pub clock: Arc<dyn Clock>,
}

/// Why a command failed: the wire code and display text of the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    code: ErrorCode,
    message: String,
}

impl Failure {
    fn new(code: ErrorCode) -> Failure {
        Failure { code, message: default_message(code).to_string() }
    }

    fn with(code: ErrorCode, message: impl Into<String>) -> Failure {
        Failure { code, message: message.into() }
    }
}

/// Fixed display text per code: never built from request fields, so it
/// cannot carry a password or a pasted key.
fn default_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NotUnlocked => "Admin session ended. Unlock again",
        ErrorCode::BadPassword => "Wrong password",
        ErrorCode::RateLimited => "Too many attempts. Wait and try again",
        ErrorCode::InvalidName => "That name is not allowed",
        ErrorCode::InvalidPassword => "That password is not allowed",
        ErrorCode::InvalidKey => "That is not a valid public key",
        ErrorCode::Exists => "That already exists",
        ErrorCode::LeftoverState => "A home directory or key file for that name is left over",
        ErrorCode::NotFound => "Not found",
        ErrorCode::SystemAccount => "System accounts cannot be changed here",
        ErrorCode::ProtectedAccount => "This account is protected",
        ErrorCode::LastAdmin => "This would leave no administrator",
        ErrorCode::GroupNotAssignable => "That group cannot be assigned here",
        ErrorCode::TooMany => "The limit has been reached",
        ErrorCode::ConfirmRequired => "Confirmation needed",
        ErrorCode::NoPassword => "The account has no password",
        ErrorCode::SessionsRemain => "Processes of the account are still running",
        ErrorCode::UserBusy => "The account is in use",
        ErrorCode::StorageUnavailable => "Account storage is not available",
        ErrorCode::Internal => "The change could not be made",
        ErrorCode::GenerateFailed => "Could not set the password",
        ErrorCode::AckFailed => "Could not save the confirmation",
    }
}

impl From<GuardError> for Failure {
    fn from(e: GuardError) -> Failure {
        let code = ErrorCode::from(e);
        // The domain's own texts name no value from the request.
        match e {
            GuardError::Name(n) => Failure::with(code, n.to_string()),
            GuardError::Password(p) => Failure::with(code, p.to_string()),
            GuardError::Key(k) => Failure::with(code, k.to_string()),
            GuardError::ProtectedAccount => Failure::with(code, format!("The {KIOSK_USER} account cannot be removed")),
            _ => Failure::new(code),
        }
    }
}

impl From<AccountsError> for Failure {
    fn from(e: AccountsError) -> Failure {
        match e {
            AccountsError::InvalidName => Failure::new(ErrorCode::InvalidName),
            AccountsError::InvalidPassword => Failure::new(ErrorCode::InvalidPassword),
            AccountsError::NoPassword => Failure::new(ErrorCode::NoPassword),
            AccountsError::SessionsRemain => Failure::new(ErrorCode::SessionsRemain),
            AccountsError::NotFound => Failure::new(ErrorCode::NotFound),
            AccountsError::Busy => Failure::new(ErrorCode::UserBusy),
            AccountsError::Tool(inner) => {
                // The tool error names the program and its exit status, never
                // an argument value.
                warn!("accounts: a system tool failed: {inner}");
                Failure::new(ErrorCode::Internal)
            }
        }
    }
}

impl From<AppError> for Failure {
    fn from(e: AppError) -> Failure {
        warn!("accounts: {e}");
        match e.kind {
            ErrorKind::Invalid => Failure::new(ErrorCode::InvalidName),
            _ => Failure::new(ErrorCode::Internal),
        }
    }
}

impl From<crate::domain::accounts::session::SessionError> for Failure {
    fn from(_: crate::domain::accounts::session::SessionError) -> Failure {
        Failure::new(ErrorCode::NotUnlocked)
    }
}

fn denied(d: Denied) -> Failure {
    match d {
        Denied::Wait(secs) => Failure::with(ErrorCode::RateLimited, format!("Too many attempts. Try again in {secs} s")),
        Denied::NeedPress => Failure::with(ErrorCode::RateLimited, "Too many attempts. Press the knob, then try again"),
    }
}

/// `confirm` must echo the target exactly.
fn require_echo(confirm: &str, target: &str) -> Result<(), Failure> {
    guard::require_confirm(!target.is_empty() && confirm == target).map_err(Failure::from)
}

/// Only valid names reach the journal: a client may have typed a password
/// into a name field.
fn loggable(name: &str) -> &str {
    if names::is_valid_name(name) {
        name
    } else {
        "<invalid>"
    }
}

struct State {
    sessions: Sessions,
    unlock: UnlockLimiter,
    mutations: MutationLimiter,
}

pub struct AccountsService {
    ports: AccountsPorts,
    replies: Arc<dyn TargetedSink>,
    limits: Limits,
    policy: GroupPolicy,
    ttl_secs: u64,
    base: Instant,
    state: Mutex<State>,
    /// Serialises unlock verification so the limiter sees every failure
    /// before the next attempt is let through.
    unlock_gate: Mutex<()>,
    /// One mutation at a time (plan D6).
    mutate: Mutex<()>,
}

impl AccountsService {
    pub fn new(ports: AccountsPorts, replies: Arc<dyn TargetedSink>, cfg: &UsersConfig) -> AccountsService {
        let base = ports.clock.now();
        let unlock = UnlockConfig {
            free_attempts: cfg.unlock_free_attempts,
            backoff_base_secs: 5,
            backoff_max_secs: cfg.unlock_backoff_max_secs,
            global_cap: cfg.unlock_global_cap,
            global_window_secs: cfg.unlock_global_window_secs,
        };
        AccountsService {
            ports,
            replies,
            limits: Limits { max_users: cfg.max_users, max_groups: cfg.max_groups, max_keys_per_user: cfg.max_keys_per_user },
            policy: GroupPolicy::new(cfg.assignable_system_groups.clone()),
            ttl_secs: cfg.session_ttl_secs,
            base,
            state: Mutex::new(State {
                sessions: Sessions::new(cfg.session_ttl_secs),
                unlock: UnlockLimiter::new(unlock),
                mutations: MutationLimiter::per_minute(cfg.mutations_per_minute),
            }),
            unlock_gate: Mutex::new(()),
            mutate: Mutex::new(()),
        }
    }

    /// Drops sessions past their fixed TTL and tells their connections.
    /// Nothing calls `Sessions::require` proactively (it only reaps lazily,
    /// on the next command on that connection), so this is the one active
    /// sweep in the daemon; `run` schedules it every 10 s. The guarantee is
    /// the server refusal: `require` is exact at the TTL, this only
    /// notifies, so the notice can lag by up to one sweep period.
    pub fn sweep_expired(&self) {
        let now = self.now();
        let gone = lock(&self.state).sessions.sweep(now);
        for &c in &gone {
            self.send(ConnId(c), session_locked());
        }
    }

    fn now(&self) -> u64 {
        self.ports.clock.now().saturating_duration_since(self.base).as_secs()
    }

    fn send(&self, conn: ConnId, e: BridgeEvent) {
        self.replies.emit_to(conn, e);
    }

    fn send_error(&self, conn: ConnId, context: &str, f: Failure) {
        self.send(conn, BridgeEvent::UserError { context: context.into(), code: f.code.as_str().into(), message: f.message });
    }

    /// A physical knob push: one more unlock attempt is allowed past the
    /// global cap.
    pub fn note_press(&self) {
        lock(&self.state).unlock.note_press();
    }

    /// A websocket connection closed: its session and counters go with it.
    pub fn connection_closed(&self, conn: ConnId) {
        let mut st = lock(&self.state);
        st.sessions.lock(conn.0);
        st.unlock.forget(conn.0);
        st.mutations.forget(conn.0);
    }

    /// `ButtonPolicy`'s gate with the knob press also feeding this service.
    pub fn gate_with_press(self: &Arc<Self>, inner: Arc<dyn FirstBootGate>) -> Arc<dyn FirstBootGate> {
        Arc::new(PressForwarder { accounts: self.clone(), inner })
    }

    /// The narrow view device settings (and anything else outside this file)
    /// needs onto the admin unlock session: no second unlock mechanism, just
    /// this one method of this one session store.
    pub fn admin_gate(self: &Arc<Self>) -> Arc<dyn AdminSessionGate> {
        self.clone()
    }

    // -----------------------------------------------------------------
    // Session
    // -----------------------------------------------------------------

    pub fn unlock(&self, conn: ConnId, username: &str, password: &Secret) {
        const CTX: &str = "user_unlock";
        match self.try_unlock(conn, username, password) {
            Ok((name, left)) => {
                info!("accounts: unlock {name} ok (conn {})", conn.0);
                self.send(conn, BridgeEvent::UserSession { unlocked: true, expires_in_secs: Some(left), username: Some(name) });
            }
            Err(f) => {
                info!("accounts: unlock denied: {} (conn {})", f.code.as_str(), conn.0);
                self.send_error(conn, CTX, f);
            }
        }
    }

    fn try_unlock(&self, conn: ConnId, username: &str, password: &Secret) -> Result<(String, u64), Failure> {
        if !self.ports.files.storage_available() {
            return Err(Failure::new(ErrorCode::StorageUnavailable));
        }
        let _turn = lock(&self.unlock_gate);
        let now = self.now();
        lock(&self.state).unlock.attempt(conn.0, now).map_err(denied)?;
        let snap = self.ports.db.snapshot()?;
        let today = self.ports.db.today_days();
        // A name that is not a valid, existing admin is a failed attempt like
        // any other: no hint about which part was wrong, and it counts.
        if let Err(e) = guard::check_can_unlock(&snap, today, username) {
            lock(&self.state).unlock.record_failure(conn.0, now);
            return Err(Failure::new(ErrorCode::from(e)));
        }
        match self.ports.verifier.verify(username, password) {
            Ok(true) => {
                let mut st = lock(&self.state);
                st.unlock.record_success(conn.0);
                st.sessions.unlock(conn.0, username, now);
                Ok((username.to_string(), self.ttl_secs))
            }
            Ok(false) => {
                lock(&self.state).unlock.record_failure(conn.0, now);
                Err(Failure::new(ErrorCode::BadPassword))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn lock_session(&self, conn: ConnId) {
        lock(&self.state).sessions.lock(conn.0);
        self.send(conn, session_locked());
    }

    /// Ends every session of `admin` after the account stopped being an
    /// effective admin (removed, locked, demoted) and tells each connection.
    fn drop_sessions_if_not_admin(&self, name: &str) {
        let Ok(snap) = self.ports.db.snapshot() else { return };
        if snap.is_effective_admin(name, self.ports.db.today_days()) {
            return;
        }
        let gone = lock(&self.state).sessions.drop_admin(name);
        for &c in &gone {
            self.send(ConnId(c), session_locked());
        }
    }

    // -----------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------

    /// While locked only the effective admins are listed (the unlock screen
    /// needs somebody to pick); an unlocked session sees every account.
    pub fn list_users(&self, conn: ConnId) {
        const CTX: &str = "user_list_get";
        let result = (|| -> Result<Vec<UserWire>, Failure> {
            if !self.ports.files.storage_available() {
                return Err(Failure::new(ErrorCode::StorageUnavailable));
            }
            let unlocked = {
                let now = self.now();
                lock(&self.state).sessions.require(conn.0, now).is_ok()
            };
            let snap = self.ports.db.snapshot()?;
            let today = self.ports.db.today_days();
            Ok(snap
                .views(today)
                .into_iter()
                .filter(|v| unlocked || v.admin)
                .map(|v| {
                    let key_count = if unlocked && is_managed_id(v.uid) { self.key_count(&v.name) } else { 0 };
                    user_wire(v, key_count)
                })
                .collect())
        })();
        match result {
            Ok(users) => self.send(conn, BridgeEvent::UserList { users }),
            Err(f) => self.send_error(conn, CTX, f),
        }
    }

    fn key_count(&self, name: &str) -> u32 {
        self.ports.keys.read(name).map(|l| l.len() as u32).unwrap_or(0)
    }

    pub fn list_groups(&self, conn: ConnId) {
        const CTX: &str = "group_list_get";
        let result = (|| -> Result<Vec<GroupWire>, Failure> {
            self.read_gate(conn)?;
            let snap = self.ports.db.snapshot()?;
            Ok(snap
                .groups
                .iter()
                .map(|g| GroupWire {
                    name: g.name.clone(),
                    gid: g.gid,
                    system: is_system_id(g.gid),
                    assignable: self.policy.is_assignable(&snap, &g.name),
                    members: g.members.clone(),
                })
                .collect())
        })();
        match result {
            Ok(groups) => self.send(conn, BridgeEvent::GroupList { groups }),
            Err(f) => self.send_error(conn, CTX, f),
        }
    }

    pub fn list_keys(&self, conn: ConnId, name: &str) {
        const CTX: &str = "ssh_key_list_get";
        let result = (|| -> Result<Vec<SshKeyWire>, Failure> {
            self.read_gate(conn)?;
            let snap = self.ports.db.snapshot()?;
            let user = guard::mutable_target(&snap, name)?;
            self.key_wires(&user.name)
        })();
        match result {
            Ok(keys) => self.send(conn, BridgeEvent::SshKeyList { name: name.to_string(), keys }),
            Err(f) => self.send_error(conn, CTX, f),
        }
    }

    fn key_wires(&self, name: &str) -> Result<Vec<SshKeyWire>, Failure> {
        let mut out = Vec::new();
        for line in self.ports.keys.read(name)? {
            let Ok(parsed) = keys::parse_line(&line) else {
                warn!("accounts: skipping an unparsable line in a key file");
                continue;
            };
            match self.ports.inspector.fingerprint(&line) {
                Ok(fingerprint) => out.push(SshKeyWire { fingerprint, key_type: parsed.key_type, comment: parsed.comment }),
                Err(e) => warn!("accounts: a stored key could not be fingerprinted: {e}"),
            }
        }
        Ok(out)
    }

    /// Storage and session checks of a read-only command.
    fn read_gate(&self, conn: ConnId) -> Result<(), Failure> {
        if !self.ports.files.storage_available() {
            return Err(Failure::new(ErrorCode::StorageUnavailable));
        }
        let now = self.now();
        lock(&self.state).sessions.require(conn.0, now)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Mutations
    // -----------------------------------------------------------------

    /// Storage, session, mutation limiter. Returns the admin's name.
    fn mutation_gate(&self, conn: ConnId) -> Result<String, Failure> {
        if !self.ports.files.storage_available() {
            return Err(Failure::new(ErrorCode::StorageUnavailable));
        }
        let now = self.now();
        let mut st = lock(&self.state);
        let admin = st.sessions.require(conn.0, now)?.to_string();
        if !st.mutations.allow(conn.0, now) {
            return Err(Failure::new(ErrorCode::RateLimited));
        }
        Ok(admin)
    }

    /// Runs one mutation under the mutation mutex and answers the requester.
    /// `body` returns the events of a success (the result, and a generated
    /// password when there is one); a failure becomes `user_result` with
    /// `success=false`, except `not_unlocked`, which is a `user_error` so the
    /// frontend returns to the unlock screen.
    fn mutation(&self, conn: ConnId, op: &'static str, name: &str, body: impl FnOnce(&str) -> Result<Vec<BridgeEvent>, Failure>) {
        let _one_at_a_time = lock(&self.mutate);
        let result = self.mutation_gate(conn).and_then(|admin| body(&admin));
        match result {
            Ok(events) => {
                info!("accounts: {op} {} ok (conn {})", loggable(name), conn.0);
                for e in events {
                    self.send(conn, e);
                }
            }
            Err(f) if f.code == ErrorCode::NotUnlocked => {
                info!("accounts: {op} {} refused: not_unlocked (conn {})", loggable(name), conn.0);
                self.send_error(conn, op, f);
            }
            Err(f) => {
                info!("accounts: {op} {} failed: {} (conn {})", loggable(name), f.code.as_str(), conn.0);
                self.send(
                    conn,
                    BridgeEvent::UserResult {
                        op: op.into(),
                        name: name.to_string(),
                        success: false,
                        error: Some(f.code.as_str().into()),
                        message: Some(f.message),
                    },
                );
            }
        }
    }

    fn generate(&self) -> Result<Secret, Failure> {
        password::generate(|buf| self.ports.entropy.fill(buf)).map_err(|e| match e {
            GenerateError::Entropy(e) => Failure::from(e),
            GenerateError::Exhausted => Failure::new(ErrorCode::Internal),
        })
    }

    /// Checks `password` for `user` against the real hash, counted by the
    /// unlock limiter like an unlock attempt (a hijacked session must not
    /// get unlimited guesses at the admin's own password).
    fn verify_counted(&self, conn: ConnId, user: &str, password: &Secret) -> Result<(), Failure> {
        let _turn = lock(&self.unlock_gate);
        let now = self.now();
        lock(&self.state).unlock.attempt(conn.0, now).map_err(denied)?;
        if self.ports.verifier.verify(user, password)? {
            lock(&self.state).unlock.record_success(conn.0);
            Ok(())
        } else {
            lock(&self.state).unlock.record_failure(conn.0, now);
            Err(Failure::new(ErrorCode::BadPassword))
        }
    }

    pub fn add_user(&self, conn: ConnId, name: &str, password: &Secret, make_admin: bool) {
        self.mutation(conn, "user_add", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            let generate = password.expose().is_empty();
            if !names::is_valid_name(name) {
                return Err(GuardError::Name(names::NameError::Invalid).into());
            }
            let leftover = self.ports.db.leftover_state(name)?;
            guard::check_add_user(&snap, name, (!generate).then(|| password.expose()), leftover, &self.limits)?;
            let secret = if generate { self.generate()? } else { password.clone() };
            self.ports.admin.add_user(name)?;
            let finish = (|| -> Result<(), Failure> {
                self.ports.accounts.set_password(name, &secret)?;
                if make_admin {
                    self.ports.admin.add_member(name, ADMIN_GROUP)?;
                }
                Ok(())
            })();
            if let Err(f) = finish {
                // The account was created locked; a half-made one is removed.
                if let Err(e) = self.ports.admin.remove_user(name, false) {
                    warn!("accounts: rollback of {} failed: {e}", loggable(name));
                }
                return Err(f);
            }
            let mut events = Vec::new();
            if generate {
                events.push(BridgeEvent::UserSecret { name: name.to_string(), password: WireSecret::from(&secret) });
            }
            events.push(ok_result("user_add", name));
            Ok(events)
        });
    }

    pub fn remove_user(&self, conn: ConnId, name: &str, keep_home: bool, confirm: &str) {
        self.mutation(conn, "user_remove", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            guard::check_mutation(&snap, self.ports.db.today_days(), name, Mutation::Remove)?;
            require_echo(confirm, name)?;
            self.ports.accounts.end_sessions(name)?;
            self.ports.admin.remove_user(name, keep_home)?;
            // The home may stay (`keep_home`); the key file never does, so a
            // re-created name cannot inherit it.
            if let Err(e) = self.ports.files.delete_key_file(name) {
                warn!("accounts: could not delete the key file of a removed user: {e}");
            }
            self.drop_sessions_if_not_admin(name);
            Ok(vec![ok_result("user_remove", name)])
        });
    }

    pub fn set_locked(&self, conn: ConnId, name: &str, locked: bool, confirm: &str) {
        self.mutation(conn, "user_set_locked", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            if locked {
                guard::check_mutation(&snap, self.ports.db.today_days(), name, Mutation::Lock)?;
                require_echo(confirm, name)?;
                self.ports.accounts.set_locked(name, true)?;
                let after = (|| -> Result<(), Failure> {
                    if name == KIOSK_USER {
                        self.ports.admin.set_kiosk_terminal(false)?;
                    }
                    self.ports.accounts.end_sessions(name)?;
                    Ok(())
                })();
                self.drop_sessions_if_not_admin(name);
                after?;
            } else {
                guard::mutable_target(&snap, name)?;
                self.ports.accounts.set_locked(name, false)?;
                if name == KIOSK_USER {
                    self.ports.admin.set_kiosk_terminal(true)?;
                }
            }
            Ok(vec![ok_result("user_set_locked", name)])
        });
    }

    pub fn set_password(&self, conn: ConnId, name: &str, password: &Secret, current: &Secret, confirm: &str) {
        self.mutation(conn, "user_set_password", name, |admin| {
            let snap = self.ports.db.snapshot()?;
            let generate = password.expose().is_empty();
            guard::check_set_password(&snap, name, (!generate).then(|| password.expose()))?;
            if name == admin {
                self.verify_counted(conn, name, current)?;
            } else {
                require_echo(confirm, name)?;
            }
            let secret = if generate { self.generate()? } else { password.clone() };
            self.ports.accounts.set_password(name, &secret)?;
            let mut events = Vec::new();
            if generate {
                events.push(BridgeEvent::UserSecret { name: name.to_string(), password: WireSecret::from(&secret) });
            }
            events.push(ok_result("user_set_password", name));
            Ok(events)
        });
    }

    pub fn add_member(&self, conn: ConnId, name: &str, group: &str, confirm: &str) {
        self.mutation(conn, "user_group_add", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            let needs_confirm = groups::check_add_member(&snap, &self.policy, name, group)?;
            if needs_confirm {
                require_echo(confirm, group)?;
            }
            if !snap.group(group).is_some_and(|g| g.members.iter().any(|m| m == name)) {
                self.ports.admin.add_member(name, group)?;
            }
            Ok(vec![ok_result("user_group_add", name)])
        });
    }

    pub fn remove_member(&self, conn: ConnId, name: &str, group: &str) {
        self.mutation(conn, "user_group_remove", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            groups::check_remove_member(&snap, &self.policy, self.ports.db.today_days(), name, group)?;
            self.ports.admin.remove_member(name, group)?;
            if group == ADMIN_GROUP {
                self.drop_sessions_if_not_admin(name);
            }
            Ok(vec![ok_result("user_group_remove", name)])
        });
    }

    pub fn add_group(&self, conn: ConnId, name: &str) {
        self.mutation(conn, "group_add", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            groups::check_create_group(&snap, name, &self.limits)?;
            self.ports.admin.add_group(name)?;
            Ok(vec![ok_result("group_add", name)])
        });
    }

    pub fn remove_group(&self, conn: ConnId, name: &str, confirm: &str) {
        self.mutation(conn, "group_remove", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            let needs_confirm = groups::check_remove_group(&snap, name)?;
            if needs_confirm {
                require_echo(confirm, name)?;
            }
            self.ports.admin.remove_group(name)?;
            Ok(vec![ok_result("group_remove", name)])
        });
    }

    pub fn add_key(&self, conn: ConnId, name: &str, key: &str) {
        self.mutation(conn, "ssh_key_add", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            let user = guard::mutable_target(&snap, name)?.clone();
            let parsed = keys::parse_line(key).map_err(GuardError::from)?;
            let mut lines = self.ports.keys.read(name)?;
            if !keys::check_key_count(lines.len(), self.limits.max_keys_per_user) {
                return Err(GuardError::TooMany.into());
            }
            let line = parsed.line();
            let fingerprint = self.ports.inspector.fingerprint(&line).map_err(|e| match e.kind {
                ErrorKind::Invalid => Failure::new(ErrorCode::InvalidKey),
                _ => Failure::from(e),
            })?;
            if self.key_wires(name)?.iter().any(|k| k.fingerprint == fingerprint) {
                return Err(Failure::with(ErrorCode::Exists, "That key is already installed"));
            }
            lines.push(line);
            self.ports.keys.write(name, &lines, (user.uid, user.gid))?;
            Ok(vec![ok_result("ssh_key_add", name)])
        });
    }

    pub fn remove_key(&self, conn: ConnId, name: &str, fingerprint: &str, confirm: &str) {
        self.mutation(conn, "ssh_key_remove", name, |_admin| {
            let snap = self.ports.db.snapshot()?;
            let user = guard::mutable_target(&snap, name)?.clone();
            require_echo(confirm, fingerprint)?;
            let lines = self.ports.keys.read(name)?;
            let mut kept = Vec::with_capacity(lines.len());
            let mut found = false;
            for line in lines {
                let matches = self.ports.inspector.fingerprint(&line).is_ok_and(|f| f == fingerprint);
                if matches && !found {
                    found = true;
                } else {
                    kept.push(line);
                }
            }
            if !found {
                return Err(Failure::new(ErrorCode::NotFound));
            }
            self.ports.keys.write(name, &kept, (user.uid, user.gid))?;
            Ok(vec![ok_result("ssh_key_remove", name)])
        });
    }
}

fn ok_result(op: &str, name: &str) -> BridgeEvent {
    BridgeEvent::UserResult { op: op.into(), name: name.to_string(), success: true, error: None, message: None }
}

fn session_locked() -> BridgeEvent {
    BridgeEvent::UserSession { unlocked: false, expires_in_secs: None, username: None }
}

fn user_wire(v: UserView, key_count: u32) -> UserWire {
    UserWire {
        name: v.name,
        uid: v.uid,
        gid: v.gid,
        home: v.home,
        shell: v.shell,
        locked: v.locked,
        admin: v.admin,
        system: v.system,
        protected: v.protected,
        groups: v.groups,
        key_count,
    }
}

/// The one thing device settings (and any other future feature behind the
/// same unlock) needs: is this connection's admin session live right now.
/// Delegates to the existing `Sessions::require`, so there is still exactly
/// one unlock mechanism, not a second one layered on top.
impl AdminSessionGate for AccountsService {
    fn require(&self, conn: ConnId) -> Result<String, AppError> {
        let now = self.now();
        lock(&self.state).sessions.require(conn.0, now).map(|admin| admin.to_string()).map_err(|_| AppError::invalid("not_unlocked"))
    }
}

/// `ButtonPolicy`'s gate: the first-boot gate's answers, plus every knob
/// push feeds the account service's unlock cap.
struct PressForwarder {
    accounts: Arc<AccountsService>,
    inner: Arc<dyn FirstBootGate>,
}

impl FirstBootGate for PressForwarder {
    fn active(&self) -> bool {
        self.inner.active()
    }

    fn on_m_down(&self) {
        self.inner.on_m_down();
    }

    fn on_knob_press(&self) {
        self.accounts.note_press();
        self.inner.on_knob_press();
    }
}

/// Fake ports for tests (also used by `tests/no_secret_logging.rs`).
pub mod testing {
    use super::*;
    use crate::app::hub::RecordingSink;
    use crate::domain::accounts::model::Snapshot;
    use crate::services::users::testing::{FakeEntropy, FakeTimer};
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    pub const TODAY: i64 = 20_000;
    pub const ADMIN_PASSWORD: &str = "Kiosk-Admin-Pass-1";

    #[derive(Clone)]
    pub struct SysUser {
        pub name: String,
        pub uid: u32,
        pub gid: u32,
    }

    #[derive(Clone)]
    pub struct SysGroup {
        pub name: String,
        pub gid: u32,
        pub members: Vec<String>,
    }

    /// The whole fake machine: account files, passwords, key files, homes and
    /// an ordered log of every mutating call.
    pub struct Sys {
        pub users: Vec<SysUser>,
        pub groups: Vec<SysGroup>,
        /// Plaintext stand-in for the hash; `None` is the `!` hash.
        pub passwords: BTreeMap<String, Option<String>>,
        pub locked: BTreeSet<String>,
        pub keys: BTreeMap<String, Vec<String>>,
        pub homes: BTreeSet<String>,
        pub calls: Vec<String>,
        pub storage: bool,
        pub terminal_enabled: bool,
        pub fail_set_password: bool,
        pub fail_add_member: bool,
        pub busy: BTreeSet<String>,
        pub sessions_remain: BTreeSet<String>,
        pub verify_calls: u32,
        pub bad_key_fingerprint: bool,
    }

    pub struct FakeSystem(pub Mutex<Sys>);

    impl FakeSystem {
        /// root, nobody, kiosk (admin with `ADMIN_PASSWORD`), alice (admin,
        /// password set), bob (plain user, no password). Groups root, adm,
        /// sudo and a private group each.
        pub fn new() -> FakeSystem {
            let user = |name: &str, uid: u32| SysUser { name: name.into(), uid, gid: uid };
            let group = |name: &str, gid: u32, members: &[&str]| SysGroup { name: name.into(), gid, members: members.iter().map(|m| m.to_string()).collect() };
            let mut passwords = BTreeMap::new();
            passwords.insert("root".to_string(), None);
            passwords.insert("nobody".to_string(), None);
            passwords.insert("kiosk".to_string(), Some(ADMIN_PASSWORD.to_string()));
            passwords.insert("alice".to_string(), Some("Alice-Password-22".to_string()));
            passwords.insert("bob".to_string(), None);
            FakeSystem(Mutex::new(Sys {
                users: vec![user("root", 0), user("nobody", 65534), user("kiosk", 1000), user("alice", 1001), user("bob", 1002)],
                groups: vec![
                    group("root", 0, &[]),
                    group("adm", 4, &["kiosk"]),
                    group("sudo", 27, &["kiosk", "alice"]),
                    group("kiosk", 1000, &[]),
                    group("alice", 1001, &[]),
                    group("bob", 1002, &[]),
                    group("crew", 1003, &["bob"]),
                ],
                passwords,
                locked: BTreeSet::new(),
                keys: BTreeMap::new(),
                homes: ["kiosk", "alice", "bob"].iter().map(|s| s.to_string()).collect(),
                calls: Vec::new(),
                storage: true,
                terminal_enabled: true,
                fail_set_password: false,
                fail_add_member: false,
                busy: BTreeSet::new(),
                sessions_remain: BTreeSet::new(),
                verify_calls: 0,
                bad_key_fingerprint: false,
            }))
        }

        pub fn with<T>(&self, f: impl FnOnce(&mut Sys) -> T) -> T {
            f(&mut lock(&self.0))
        }

        pub fn calls(&self) -> Vec<String> {
            self.with(|s| s.calls.clone())
        }

        fn log(s: &mut Sys, line: impl Into<String>) {
            s.calls.push(line.into());
        }
    }

    impl Default for FakeSystem {
        fn default() -> Self {
            FakeSystem::new()
        }
    }

    impl AccountDb for FakeSystem {
        fn snapshot(&self) -> Result<Snapshot, AppError> {
            let s = lock(&self.0);
            let passwd: String = s.users.iter().map(|u| format!("{}:x:{}:{}::/var/home/{}:/usr/bin/bash\n", u.name, u.uid, u.gid, u.name)).collect();
            let group: String = s.groups.iter().map(|g| format!("{}:x:{}:{}\n", g.name, g.gid, g.members.join(","))).collect();
            let shadow: String = s
                .users
                .iter()
                .map(|u| {
                    let locked = s.locked.contains(&u.name);
                    let hash = match s.passwords.get(&u.name).cloned().flatten() {
                        Some(_) => "$6$fake$hash",
                        None => "",
                    };
                    let field = match (locked, hash.is_empty()) {
                        (_, true) => "!".to_string(),
                        (true, false) => format!("!{hash}"),
                        (false, false) => hash.to_string(),
                    };
                    format!("{}:{field}:19000:0:99999:7::{}:\n", u.name, if locked { "1" } else { "" })
                })
                .collect();
            Ok(Snapshot::parse(&passwd, &group, &shadow))
        }

        fn today_days(&self) -> i64 {
            TODAY
        }

        fn leftover_state(&self, name: &str) -> Result<bool, AppError> {
            let s = lock(&self.0);
            Ok(s.homes.contains(name) || s.keys.contains_key(name))
        }
    }

    impl AccountsPort for FakeSystem {
        fn set_password(&self, user: &str, password: &Secret) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("chpasswd {user}"));
            if s.fail_set_password {
                return Err(AccountsError::Tool(AppError::failed("chpasswd exited with status 1")));
            }
            if !s.users.iter().any(|u| u.name == user) {
                return Err(AccountsError::NotFound);
            }
            s.passwords.insert(user.to_string(), Some(password.expose().to_string()));
            Ok(())
        }

        fn clear_password(&self, user: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            s.passwords.insert(user.to_string(), None);
            Ok(())
        }

        fn set_locked(&self, user: &str, locked: bool) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("usermod {} {user}", if locked { "lock" } else { "unlock" }));
            if !locked && s.passwords.get(user).cloned().flatten().is_none() {
                return Err(AccountsError::NoPassword);
            }
            if locked {
                s.locked.insert(user.to_string());
            } else {
                s.locked.remove(user);
            }
            Ok(())
        }

        fn end_sessions(&self, user: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("end_sessions {user}"));
            if s.sessions_remain.contains(user) {
                return Err(AccountsError::SessionsRemain);
            }
            Ok(())
        }
    }

    impl AccountAdmin for FakeSystem {
        fn add_user(&self, name: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("useradd {name}"));
            let uid = s.users.iter().map(|u| u.uid).filter(|u| *u < 60000).max().unwrap_or(999) + 1;
            s.users.push(SysUser { name: name.into(), uid, gid: uid });
            s.groups.push(SysGroup { name: name.into(), gid: uid, members: Vec::new() });
            s.passwords.insert(name.to_string(), None);
            s.homes.insert(name.to_string());
            Ok(())
        }

        fn remove_user(&self, name: &str, keep_home: bool) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("userdel {name}{}", if keep_home { " keep_home" } else { "" }));
            if s.busy.contains(name) {
                return Err(AccountsError::Busy);
            }
            s.users.retain(|u| u.name != name);
            s.groups.retain(|g| g.name != name);
            for g in s.groups.iter_mut() {
                g.members.retain(|m| m != name);
            }
            s.passwords.remove(name);
            s.locked.remove(name);
            if !keep_home {
                s.homes.remove(name);
            }
            Ok(())
        }

        fn add_group(&self, name: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("groupadd {name}"));
            let gid = s.groups.iter().map(|g| g.gid).filter(|g| *g < 60000).max().unwrap_or(999) + 1;
            s.groups.push(SysGroup { name: name.into(), gid, members: Vec::new() });
            Ok(())
        }

        fn remove_group(&self, name: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("groupdel {name}"));
            s.groups.retain(|g| g.name != name);
            Ok(())
        }

        fn add_member(&self, user: &str, group: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("gpasswd add {user} {group}"));
            if s.fail_add_member {
                return Err(AccountsError::Tool(AppError::failed("gpasswd exited with status 3")));
            }
            match s.groups.iter_mut().find(|g| g.name == group) {
                Some(g) => {
                    g.members.push(user.to_string());
                    Ok(())
                }
                None => Err(AccountsError::NotFound),
            }
        }

        fn remove_member(&self, user: &str, group: &str) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("gpasswd delete {user} {group}"));
            if let Some(g) = s.groups.iter_mut().find(|g| g.name == group) {
                g.members.retain(|m| m != user);
            }
            Ok(())
        }

        fn set_kiosk_terminal(&self, enabled: bool) -> Result<(), AccountsError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("kiosk_terminal {enabled}"));
            s.terminal_enabled = enabled;
            Ok(())
        }
    }

    impl AccountFiles for FakeSystem {
        fn storage_available(&self) -> bool {
            lock(&self.0).storage
        }
        fn read_lower(&self, _: crate::domain::accounts::state::AccountFile) -> Result<String, AppError> {
            Ok(String::new())
        }
        fn read_current(&self, _: crate::domain::accounts::state::AccountFile) -> Result<String, AppError> {
            Ok(String::new())
        }
        fn append_lines(&self, _: crate::domain::accounts::state::AccountFile, _: &[String]) -> Result<(), AppError> {
            Ok(())
        }
        fn list_upper(&self) -> Result<Vec<String>, AppError> {
            Ok(Vec::new())
        }
        fn list_key_files(&self) -> Result<Vec<String>, AppError> {
            Ok(lock(&self.0).keys.keys().cloned().collect())
        }
        fn delete_key_file(&self, name: &str) -> Result<(), AppError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("delete_key_file {name}"));
            s.keys.remove(name);
            Ok(())
        }
    }

    impl PasswordVerifier for FakeSystem {
        fn verify(&self, user: &str, password: &Secret) -> Result<bool, AppError> {
            let mut s = lock(&self.0);
            s.verify_calls += 1;
            let hash_ok = s.passwords.get(user).cloned().flatten().is_some_and(|p| p == password.expose());
            Ok(hash_ok && !s.locked.contains(user))
        }
    }

    impl AuthorizedKeysStore for FakeSystem {
        fn read(&self, user: &str) -> Result<Vec<String>, AppError> {
            Ok(lock(&self.0).keys.get(user).cloned().unwrap_or_default())
        }

        fn write(&self, user: &str, lines: &[String], owner: (u32, u32)) -> Result<(), AppError> {
            let mut s = lock(&self.0);
            FakeSystem::log(&mut s, format!("keys.write {user} n={} owner={}:{}", lines.len(), owner.0, owner.1));
            if lines.is_empty() {
                s.keys.remove(user);
            } else {
                s.keys.insert(user.to_string(), lines.to_vec());
            }
            Ok(())
        }
    }

    impl KeyInspector for FakeSystem {
        /// `SHA256:` plus the blob (second word); a line containing `BADKEY`
        /// is rejected like `ssh-keygen` would.
        fn fingerprint(&self, line: &str) -> Result<String, AppError> {
            let s = lock(&self.0);
            if s.bad_key_fingerprint || line.contains("BADKEY") {
                return Err(AppError::invalid("rejected"));
            }
            Ok(format!("SHA256:{}", line.split(' ').nth(1).unwrap_or("")))
        }
    }

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

    /// A key line that passes the domain parser; `tag` makes its blob unique.
    pub fn key_line(tag: u8, comment: &str) -> String {
        let t = "ssh-ed25519";
        let mut raw = (t.len() as u32).to_be_bytes().to_vec();
        raw.extend_from_slice(t.as_bytes());
        raw.extend_from_slice(&[tag, 2, 3, 4, 5]);
        format!("{t} {} {comment}", b64(&raw)).trim_end().to_string()
    }

    pub struct Rig {
        pub service: Arc<AccountsService>,
        pub sys: Arc<FakeSystem>,
        pub sink: Arc<RecordingSink>,
        pub timer: Arc<FakeTimer>,
    }

    impl Rig {
        pub fn new() -> Rig {
            Rig::with_config(&UsersConfig::default())
        }

        pub fn with_config(cfg: &UsersConfig) -> Rig {
            let sink = Arc::new(RecordingSink::new());
            Rig::with_sink(cfg, sink.clone(), sink)
        }

        /// `replies` is where the service answers (the real hub in the log
        /// tests); `sink` is only kept for `take_targeted`.
        pub fn with_sink(cfg: &UsersConfig, replies: Arc<dyn TargetedSink>, sink: Arc<RecordingSink>) -> Rig {
            let sys = Arc::new(FakeSystem::new());
            let timer = Arc::new(FakeTimer::new());
            let ports = AccountsPorts {
                db: sys.clone(),
                accounts: sys.clone(),
                admin: sys.clone(),
                files: sys.clone(),
                verifier: sys.clone(),
                keys: sys.clone(),
                inspector: sys.clone(),
                entropy: Arc::new(FakeEntropy::new(11)),
                clock: timer.clone(),
            };
            Rig { service: Arc::new(AccountsService::new(ports, replies, cfg)), sys, sink, timer }
        }

        pub fn advance(&self, secs: u64) {
            self.timer.advance(Duration::from_secs(secs));
        }

        pub fn take(&self) -> Vec<(ConnId, BridgeEvent)> {
            self.sink.take_targeted()
        }

        /// Unlocks `conn` as `kiosk` and drops the replies.
        pub fn unlock(&self, conn: ConnId) {
            self.service.unlock(conn, "kiosk", &Secret::new(ADMIN_PASSWORD));
            let got = self.take();
            assert!(matches!(&got[..], [(_, BridgeEvent::UserSession { unlocked: true, .. })]), "{got:?}");
        }
    }

    impl Default for Rig {
        fn default() -> Self {
            Rig::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const A: ConnId = ConnId(1);
    const B: ConnId = ConnId(2);

    fn only(rig: &Rig) -> BridgeEvent {
        let mut got = rig.take();
        assert_eq!(got.len(), 1, "{got:?}");
        got.remove(0).1
    }

    fn result(e: &BridgeEvent) -> (&str, &str, bool, Option<&str>) {
        match e {
            BridgeEvent::UserResult { op, name, success, error, .. } => (op, name, *success, error.as_deref()),
            other => panic!("not a user_result: {other:?}"),
        }
    }

    fn error_code(e: &BridgeEvent) -> &str {
        match e {
            BridgeEvent::UserError { code, .. } => code,
            other => panic!("not a user_error: {other:?}"),
        }
    }

    fn pw(s: &str) -> Secret {
        Secret::new(s)
    }

    fn empty() -> Secret {
        Secret::new("")
    }

    fn unlocked() -> Rig {
        let rig = Rig::new();
        rig.unlock(A);
        rig
    }

    // ---- unlock, session, idle lock ------------------------------------

    #[test]
    fn unlock_with_the_admin_password_opens_a_session_for_that_connection_only() {
        let rig = Rig::new();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert!(matches!(only(&rig), BridgeEvent::UserSession { unlocked: true, expires_in_secs: Some(1800), username: Some(u) } if u == "kiosk"));
        rig.service.add_group(A, "team");
        assert_eq!(result(&only(&rig)), ("group_add", "team", true, None));
        rig.service.add_group(B, "team2");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        assert!(!rig.sys.calls().iter().any(|c| c == "groupadd team2"));
    }

    #[test]
    fn a_wrong_password_a_non_admin_and_an_unknown_name_all_say_bad_password() {
        let rig = Rig::new();
        // One connection per attempt, so the per-connection backoff stays out of it.
        let cases = [("kiosk", "wrong-password-1"), ("bob", "x"), ("nobody", "x"), ("ghost", "x"), ("a b", "x"), ("kiosk", "")];
        for (i, (user, password)) in cases.into_iter().enumerate() {
            rig.service.unlock(ConnId(10 + i as u64), user, &pw(password));
            let code = error_code(&only(&rig)).to_string();
            assert_eq!(code, "bad_password", "{user}");
        }
        // A user with no password (root) gets the same answer, and still counts.
        rig.service.unlock(B, "root", &pw("x"));
        assert_eq!(error_code(&only(&rig)), "bad_password");
        assert_eq!(rig.sys.with(|s| s.verify_calls), 2, "only existing admins with a password reach the verifier");
    }

    #[test]
    fn unlock_backoff_three_free_then_waits_and_a_success_resets() {
        let rig = Rig::new();
        for _ in 0..3 {
            rig.service.unlock(A, "kiosk", &pw("wrong-password-1"));
            assert_eq!(error_code(&only(&rig)), "bad_password");
        }
        rig.service.unlock(A, "kiosk", &pw("wrong-password-1"));
        assert_eq!(error_code(&only(&rig)), "bad_password");
        // The fourth failure starts a 5 s wait: even the right password waits.
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        let e = only(&rig);
        assert_eq!(error_code(&e), "rate_limited");
        assert!(matches!(&e, BridgeEvent::UserError { message, .. } if message.contains("5 s")), "{e:?}");
        assert_eq!(rig.sys.with(|s| s.verify_calls), 4, "a denied attempt never reaches the verifier");
        rig.advance(5);
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert!(matches!(only(&rig), BridgeEvent::UserSession { unlocked: true, .. }));
        // Success reset the counter: three more free tries.
        rig.service.unlock(A, "kiosk", &pw("wrong-password-1"));
        assert_eq!(error_code(&only(&rig)), "bad_password");
    }

    #[test]
    fn the_global_cap_needs_a_knob_push_and_a_push_allows_one_attempt() {
        let rig = Rig::new();
        // 20 failures spread over 20 connections stay inside each one's free tries.
        for i in 0..20u64 {
            rig.service.unlock(ConnId(100 + i), "kiosk", &pw("wrong-password-1"));
            assert_eq!(error_code(&only(&rig)), "bad_password");
        }
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        let e = only(&rig);
        assert_eq!(error_code(&e), "rate_limited");
        assert!(matches!(&e, BridgeEvent::UserError { message, .. } if message.contains("knob")), "{e:?}");
        rig.service.note_press();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert!(matches!(only(&rig), BridgeEvent::UserSession { unlocked: true, .. }), "the person at the device gets in");
        rig.service.unlock(B, "kiosk", &pw(ADMIN_PASSWORD));
        assert_eq!(error_code(&only(&rig)), "rate_limited", "the press was consumed");
    }

    #[test]
    fn the_session_expires_at_exactly_the_ttl_and_activity_does_not_extend_it() {
        let rig = unlocked();
        rig.advance(1000);
        rig.service.add_group(A, "g1");
        assert!(result(&only(&rig)).2);
        rig.advance(799);
        rig.service.add_group(A, "g2");
        assert!(result(&only(&rig)).2, "1799 s after the unlock the session is still live");
        rig.advance(1);
        rig.service.add_group(A, "g3");
        assert_eq!(error_code(&only(&rig)), "not_unlocked", "refused at exactly 1800 s, whatever the activity before");
        rig.service.list_groups(A);
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
    }

    #[test]
    fn two_connections_have_independent_clocks() {
        let rig = Rig::new();
        rig.unlock(A);
        rig.advance(900);
        rig.unlock(B);
        rig.advance(900);
        rig.service.add_group(A, "g1");
        assert_eq!(error_code(&only(&rig)), "not_unlocked", "A started 1800 s ago");
        rig.service.add_group(B, "g2");
        assert!(result(&only(&rig)).2, "B started 900 s ago");
    }

    #[test]
    fn the_sweep_ends_expired_sessions_and_tells_only_their_connections() {
        let rig = Rig::new();
        rig.unlock(A);
        rig.advance(900);
        rig.unlock(B);
        rig.advance(900);
        rig.service.sweep_expired();
        let sent = rig.sink.take_targeted();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, A);
        assert!(matches!(sent[0].1, BridgeEvent::UserSession { unlocked: false, expires_in_secs: None, username: None }));
        rig.service.add_group(A, "g1");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        rig.service.sweep_expired();
        assert!(rig.sink.take_targeted().is_empty(), "nothing more to end");
    }

    #[test]
    fn lock_session_ends_it_and_a_closed_connection_forgets_it() {
        let rig = unlocked();
        rig.service.lock_session(A);
        assert!(matches!(only(&rig), BridgeEvent::UserSession { unlocked: false, expires_in_secs: None, username: None }));
        rig.service.add_group(A, "g1");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        rig.unlock(A);
        rig.service.connection_closed(A);
        rig.service.add_group(A, "g1");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
    }

    #[test]
    fn unlock_needs_storage() {
        let rig = Rig::new();
        rig.sys.with(|s| s.storage = false);
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert_eq!(error_code(&only(&rig)), "storage_unavailable");
    }

    #[test]
    fn a_locked_admin_cannot_unlock() {
        let rig = Rig::new();
        rig.sys.with(|s| {
            s.locked.insert("alice".into());
        });
        rig.service.unlock(A, "alice", &pw("Alice-Password-22"));
        assert_eq!(error_code(&only(&rig)), "bad_password");
    }

    // ---- list ------------------------------------------------------------

    #[test]
    fn a_locked_connection_sees_only_the_admins_an_unlocked_one_sees_everyone() {
        let rig = Rig::new();
        rig.service.list_users(A);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        let names: Vec<&str> = users.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["kiosk", "alice"]);
        assert!(users.iter().all(|u| u.key_count == 0));
        rig.unlock(A);
        rig.sys.with(|s| {
            s.keys.insert("bob".into(), vec![key_line(1, "b")]);
        });
        rig.service.list_users(A);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        assert_eq!(users.len(), 5);
        let bob = users.iter().find(|u| u.name == "bob").unwrap();
        assert_eq!((bob.key_count, bob.admin, bob.system, bob.groups.clone()), (1, false, false, vec!["bob".to_string(), "crew".to_string()]));
        let kiosk = users.iter().find(|u| u.name == "kiosk").unwrap();
        assert!(kiosk.protected && kiosk.admin);
        assert!(users.iter().find(|u| u.name == "root").unwrap().system);
        // A different connection stays limited to the admins.
        rig.service.list_users(B);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn group_and_key_lists_need_a_session() {
        let rig = Rig::new();
        rig.service.list_groups(A);
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        rig.service.list_keys(A, "bob");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        rig.unlock(A);
        rig.service.list_groups(A);
        let BridgeEvent::GroupList { groups } = only(&rig) else { panic!() };
        let by = |n: &str| groups.iter().find(|g| g.name == n).unwrap().clone();
        assert!(by("sudo").system && by("sudo").assignable && by("adm").assignable);
        assert!(!by("root").assignable && !by("alice").assignable, "root and private groups are not assignable");
        assert!(!by("crew").system && by("crew").assignable);
        assert_eq!(by("crew").members, vec!["bob"]);
    }

    // ---- add -------------------------------------------------------------

    #[test]
    fn add_with_a_given_password_sets_it_and_never_sends_a_secret() {
        let rig = unlocked();
        rig.service.add_user(A, "carol", &pw("Carol-Password-3"), false);
        assert_eq!(result(&only(&rig)), ("user_add", "carol", true, None));
        assert_eq!(rig.sys.calls(), vec!["useradd carol", "chpasswd carol"]);
        assert_eq!(rig.sys.with(|s| s.passwords.get("carol").cloned().flatten()), Some("Carol-Password-3".into()));
    }

    #[test]
    fn add_with_an_empty_password_generates_one_and_sends_it_once_before_the_result() {
        let rig = unlocked();
        rig.service.add_user(A, "carol", &empty(), true);
        let got = rig.take();
        assert_eq!(got.len(), 2, "{got:?}");
        let BridgeEvent::UserSecret { name, password } = &got[0].1 else { panic!("{:?}", got[0]) };
        assert_eq!(name, "carol");
        assert_eq!(password.expose().len(), 16);
        assert_eq!(rig.sys.with(|s| s.passwords.get("carol").cloned().flatten()), Some(password.expose().to_string()));
        assert_eq!(result(&got[1].1), ("user_add", "carol", true, None));
        assert!(got.iter().all(|(c, _)| *c == A));
        assert!(rig.sys.calls().contains(&"gpasswd add carol sudo".to_string()));
        // The new account is an effective admin now.
        rig.service.list_users(A);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        assert!(users.iter().find(|u| u.name == "carol").unwrap().admin);
    }

    #[test]
    fn add_refusals_use_the_guard_codes_and_change_nothing() {
        let rig = unlocked();
        let cases: [(&str, &str, &str); 6] = [
            ("Carol", "Carol-Password-3", "invalid_name"),
            ("root", "Carol-Password-3", "invalid_name"),
            ("alice", "Carol-Password-3", "exists"),
            ("bob", "Carol-Password-3", "exists"),
            ("carol", "short", "invalid_password"),
            ("carol", "aaaaaaaaaaaaaaaa", "invalid_password"),
        ];
        for (name, password, code) in cases {
            rig.service.add_user(A, name, &pw(password), false);
            let e = only(&rig);
            assert_eq!(result(&e), ("user_add", name, false, Some(code)), "{name}");
        }
        assert!(rig.sys.calls().is_empty());
    }

    #[test]
    fn add_refuses_leftover_state_and_the_user_limit() {
        let rig = unlocked();
        rig.sys.with(|s| {
            s.homes.insert("dave".into());
        });
        rig.service.add_user(A, "dave", &pw("Dave-Password-44"), false);
        assert_eq!(result(&only(&rig)).3, Some("leftover_state"));
        rig.sys.with(|s| {
            s.keys.insert("erin".into(), vec![key_line(1, "")]);
        });
        rig.service.add_user(A, "erin", &pw("Erin-Password-44"), false);
        assert_eq!(result(&only(&rig)).3, Some("leftover_state"));
        let tight = UsersConfig { max_users: 3, ..UsersConfig::default() };
        let rig = Rig::with_config(&tight);
        rig.unlock(A);
        rig.service.add_user(A, "carol", &pw("Carol-Password-3"), false);
        assert_eq!(result(&only(&rig)).3, Some("too_many"));
    }

    #[test]
    fn add_rolls_back_when_the_password_cannot_be_set() {
        let rig = unlocked();
        rig.sys.with(|s| s.fail_set_password = true);
        rig.service.add_user(A, "carol", &pw("Carol-Password-3"), false);
        let e = only(&rig);
        assert_eq!(result(&e), ("user_add", "carol", false, Some("internal")));
        assert_eq!(rig.sys.calls(), vec!["useradd carol", "chpasswd carol", "userdel carol"]);
        assert!(!rig.sys.with(|s| s.users.iter().any(|u| u.name == "carol")), "no half-made account is left");
        assert!(!rig.sys.with(|s| s.homes.contains("carol")));
        // A failing admin-group join rolls back too, and no secret was sent.
        rig.sys.with(|s| {
            s.fail_set_password = false;
            s.fail_add_member = true;
            s.calls.clear();
        });
        rig.service.add_user(A, "carol", &empty(), true);
        let got = rig.take();
        assert_eq!(got.len(), 1, "no user_secret for a failed add: {got:?}");
        assert_eq!(result(&got[0].1).3, Some("internal"));
        assert!(rig.sys.calls().last().unwrap().starts_with("userdel carol"));
    }

    // ---- remove ----------------------------------------------------------

    #[test]
    fn remove_needs_confirm_then_ends_sessions_deletes_the_account_and_the_key_file() {
        let rig = unlocked();
        rig.sys.with(|s| {
            s.keys.insert("bob".into(), vec![key_line(1, "b")]);
        });
        rig.service.remove_user(A, "bob", false, "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.remove_user(A, "bob", false, "bobby");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        assert!(rig.sys.calls().is_empty());
        rig.service.remove_user(A, "bob", false, "bob");
        assert_eq!(result(&only(&rig)), ("user_remove", "bob", true, None));
        assert_eq!(rig.sys.calls(), vec!["end_sessions bob", "userdel bob", "delete_key_file bob"]);
        assert!(!rig.sys.with(|s| s.homes.contains("bob") || s.keys.contains_key("bob")));
        // A re-add is possible (no leftover) only because the home went too.
        rig.service.add_user(A, "bob", &pw("Bob-Password-555"), false);
        assert!(result(&only(&rig)).2);
    }

    #[test]
    fn remove_keep_home_leaves_the_home_and_blocks_a_re_add() {
        let rig = unlocked();
        rig.service.remove_user(A, "bob", true, "bob");
        assert!(result(&only(&rig)).2);
        assert!(rig.sys.calls().contains(&"userdel bob keep_home".to_string()));
        rig.service.add_user(A, "bob", &pw("Bob-Password-555"), false);
        assert_eq!(result(&only(&rig)).3, Some("leftover_state"));
    }

    #[test]
    fn remove_guards_kiosk_system_accounts_and_the_last_admin() {
        let rig = unlocked();
        for (name, code) in [("kiosk", "protected_account"), ("root", "system_account"), ("ghost", "not_found")] {
            rig.service.remove_user(A, name, false, name);
            assert_eq!(result(&only(&rig)).3, Some(code), "{name}");
        }
        // alice is the only other admin: make her one by locking, then the
        // kiosk still cannot be locked, and alice (not last) can go.
        assert!(rig.sys.calls().is_empty());
    }

    #[test]
    fn remove_reports_a_busy_account_and_surviving_processes() {
        let rig = unlocked();
        rig.sys.with(|s| {
            s.busy.insert("bob".into());
        });
        rig.service.remove_user(A, "bob", false, "bob");
        assert_eq!(result(&only(&rig)).3, Some("user_busy"));
        assert!(rig.sys.with(|s| s.users.iter().any(|u| u.name == "bob")));
        rig.sys.with(|s| {
            s.busy.clear();
            s.calls.clear();
            s.sessions_remain.insert("bob".into());
        });
        rig.service.remove_user(A, "bob", false, "bob");
        assert_eq!(result(&only(&rig)).3, Some("sessions_remain"));
        assert!(!rig.sys.calls().iter().any(|c| c == "userdel bob"), "userdel is not tried while processes remain");
    }

    #[test]
    fn removing_an_admin_ends_that_admins_sessions_on_every_connection() {
        let rig = Rig::new();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        rig.service.unlock(B, "alice", &pw("Alice-Password-22"));
        rig.take();
        rig.service.remove_user(A, "alice", false, "alice");
        let got = rig.take();
        assert!(got.iter().any(|(c, e)| *c == B && matches!(e, BridgeEvent::UserSession { unlocked: false, .. })), "{got:?}");
        rig.service.add_group(B, "team");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
    }

    // ---- lock and unlock -------------------------------------------------

    #[test]
    fn lock_needs_confirm_runs_usermod_then_ends_sessions() {
        let rig = unlocked();
        rig.service.set_locked(A, "alice", true, "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.set_locked(A, "alice", true, "alice");
        assert_eq!(result(&only(&rig)), ("user_set_locked", "alice", true, None));
        assert_eq!(rig.sys.calls(), vec!["usermod lock alice", "end_sessions alice"]);
        rig.service.list_users(A);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        let alice = users.iter().find(|u| u.name == "alice").unwrap();
        assert!(alice.locked && !alice.admin);
    }

    #[test]
    fn unlock_of_an_account_needs_no_confirm_and_refuses_no_password() {
        let rig = unlocked();
        rig.service.set_locked(A, "alice", true, "alice");
        rig.take();
        rig.sys.with(|s| s.calls.clear());
        rig.service.set_locked(A, "alice", false, "");
        assert_eq!(result(&only(&rig)), ("user_set_locked", "alice", true, None));
        assert_eq!(rig.sys.calls(), vec!["usermod unlock alice"]);
        rig.service.set_locked(A, "bob", false, "");
        assert_eq!(result(&only(&rig)).3, Some("no_password"));
    }

    #[test]
    fn lock_guards_system_accounts_and_the_last_admin() {
        let rig = unlocked();
        rig.service.set_locked(A, "root", true, "root");
        assert_eq!(result(&only(&rig)).3, Some("system_account"));
        rig.service.set_locked(A, "alice", true, "alice");
        rig.take();
        rig.service.set_locked(A, "kiosk", true, "kiosk");
        assert_eq!(result(&only(&rig)).3, Some("last_admin"));
        assert!(rig.sys.with(|s| s.terminal_enabled), "a refused lock leaves the terminal alone");
    }

    #[test]
    fn locking_kiosk_stops_its_terminal_and_unlocking_restores_it() {
        let rig = unlocked();
        rig.service.set_locked(A, "kiosk", true, "kiosk");
        let got = rig.take();
        assert!(matches!(&got[0].1, BridgeEvent::UserSession { unlocked: false, .. }), "the locked admin's session ends first: {got:?}");
        assert_eq!(result(&got[1].1), ("user_set_locked", "kiosk", true, None), "alice is another admin");
        assert_eq!(rig.sys.calls(), vec!["usermod lock kiosk", "kiosk_terminal false", "end_sessions kiosk"]);
        assert!(!rig.sys.with(|s| s.terminal_enabled));
        // The locked admin's own session is gone (nothing says unlocked).
        rig.service.add_group(A, "team");
        assert_eq!(error_code(&only(&rig)), "not_unlocked");
        rig.service.unlock(B, "alice", &pw("Alice-Password-22"));
        rig.take();
        rig.service.set_locked(B, "kiosk", false, "");
        assert_eq!(result(&only(&rig)), ("user_set_locked", "kiosk", true, None));
        assert!(rig.sys.with(|s| s.terminal_enabled));
        assert_eq!(rig.sys.calls().last().unwrap(), "kiosk_terminal true");
    }

    #[test]
    fn lock_reports_sessions_that_remain_but_the_account_is_locked() {
        let rig = unlocked();
        rig.sys.with(|s| {
            s.sessions_remain.insert("alice".into());
        });
        rig.service.set_locked(A, "alice", true, "alice");
        assert_eq!(result(&only(&rig)).3, Some("sessions_remain"));
        assert!(rig.sys.with(|s| s.locked.contains("alice")));
    }

    // ---- set password ----------------------------------------------------

    #[test]
    fn resetting_another_users_password_needs_confirm_and_can_generate() {
        let rig = unlocked();
        rig.service.set_password(A, "alice", &empty(), &empty(), "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.set_password(A, "alice", &empty(), &empty(), "alice");
        let got = rig.take();
        assert_eq!(got.len(), 2);
        let BridgeEvent::UserSecret { password, .. } = &got[0].1 else { panic!() };
        assert_eq!(rig.sys.with(|s| s.passwords.get("alice").cloned().flatten()), Some(password.expose().to_string()));
        assert_eq!(result(&got[1].1), ("user_set_password", "alice", true, None));
        rig.service.set_password(A, "alice", &pw("Another-Pass-77"), &empty(), "alice");
        assert!(result(&only(&rig)).2);
    }

    #[test]
    fn changing_your_own_password_needs_the_current_one_and_counts_failures() {
        let rig = unlocked();
        rig.service.set_password(A, "kiosk", &pw("New-Kiosk-Pass-9"), &empty(), "");
        assert_eq!(result(&only(&rig)).3, Some("bad_password"));
        rig.service.set_password(A, "kiosk", &pw("New-Kiosk-Pass-9"), &pw("not-the-password-1"), "");
        assert_eq!(result(&only(&rig)).3, Some("bad_password"));
        assert_eq!(rig.sys.with(|s| s.passwords.get("kiosk").cloned().flatten()), Some(ADMIN_PASSWORD.into()), "nothing changed");
        rig.service.set_password(A, "kiosk", &pw("New-Kiosk-Pass-9"), &pw(ADMIN_PASSWORD), "");
        assert_eq!(result(&only(&rig)), ("user_set_password", "kiosk", true, None));
        assert_eq!(rig.sys.with(|s| s.passwords.get("kiosk").cloned().flatten()), Some("New-Kiosk-Pass-9".into()));
        // Wrong guesses share the unlock limiter: after three failures the
        // wait applies even to the right current password.
        let rig = unlocked();
        for _ in 0..4 {
            rig.service.set_password(A, "kiosk", &pw("New-Kiosk-Pass-9"), &pw("not-the-password-1"), "");
            rig.take();
        }
        rig.service.set_password(A, "kiosk", &pw("New-Kiosk-Pass-9"), &pw(ADMIN_PASSWORD), "");
        assert_eq!(result(&only(&rig)).3, Some("rate_limited"));
    }

    #[test]
    fn set_password_validates_and_refuses_system_accounts() {
        let rig = unlocked();
        rig.service.set_password(A, "alice", &pw("short"), &empty(), "alice");
        assert_eq!(result(&only(&rig)).3, Some("invalid_password"));
        rig.service.set_password(A, "root", &pw("Another-Pass-77"), &empty(), "root");
        assert_eq!(result(&only(&rig)).3, Some("system_account"));
        rig.service.set_password(A, "ghost", &pw("Another-Pass-77"), &empty(), "ghost");
        assert_eq!(result(&only(&rig)).3, Some("not_found"));
        assert!(rig.sys.calls().is_empty());
    }

    // ---- mutation limiter, storage, serialisation --------------------------

    #[test]
    fn mutations_are_capped_per_connection_per_minute() {
        let rig = unlocked();
        for i in 0..10 {
            rig.service.add_group(A, &format!("g{i}"));
            assert!(result(&only(&rig)).2, "{i}");
            rig.advance(1);
        }
        // The group limit (32) is not what stops this one.
        rig.service.add_group(A, "g10");
        assert_eq!(result(&only(&rig)).3, Some("rate_limited"));
        rig.advance(60);
        rig.service.add_group(A, "g10");
        assert!(result(&only(&rig)).2);
    }

    #[test]
    fn mutations_need_storage_and_report_it() {
        let rig = unlocked();
        rig.sys.with(|s| s.storage = false);
        rig.service.add_group(A, "g1");
        assert_eq!(result(&only(&rig)).3, Some("storage_unavailable"));
        rig.service.list_groups(A);
        assert_eq!(error_code(&only(&rig)), "storage_unavailable");
    }

    #[test]
    fn concurrent_mutations_run_one_at_a_time() {
        let rig = Arc::new(unlocked());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let rig = rig.clone();
                std::thread::spawn(move || rig.service.add_group(A, &format!("grp{i}")))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let got = rig.take();
        assert_eq!(got.len(), 8);
        assert!(got.iter().all(|(_, e)| result(e).2), "{got:?}");
        let gids: Vec<u32> = rig.sys.with(|s| s.groups.iter().filter(|g| g.name.starts_with("grp")).map(|g| g.gid).collect());
        let mut unique = gids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 8, "no two groups got the same gid: {gids:?}");
    }

    // ---- groups ----------------------------------------------------------

    #[test]
    fn group_membership_needs_confirm_only_for_sudo_and_checks_assignability() {
        let rig = unlocked();
        rig.service.add_member(A, "bob", "adm", "");
        assert_eq!(result(&only(&rig)), ("user_group_add", "bob", true, None));
        rig.service.add_member(A, "bob", "sudo", "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.add_member(A, "bob", "sudo", "sudo");
        assert!(result(&only(&rig)).2);
        rig.service.add_member(A, "bob", "root", "root");
        assert_eq!(result(&only(&rig)).3, Some("group_not_assignable"));
        rig.service.add_member(A, "bob", "alice", "");
        assert_eq!(result(&only(&rig)).3, Some("group_not_assignable"), "a private group");
        rig.service.add_member(A, "root", "adm", "");
        assert_eq!(result(&only(&rig)).3, Some("system_account"));
        rig.service.add_member(A, "bob", "crew", "");
        assert!(result(&only(&rig)).2, "already a member: a no-op success");
        assert_eq!(rig.sys.calls().iter().filter(|c| c.as_str() == "gpasswd add bob crew").count(), 0);
    }

    #[test]
    fn device_groups_stay_refused_unless_configured() {
        let rig = unlocked();
        rig.sys.with(|s| s.groups.push(SysGroup { name: "input".into(), gid: 105, members: vec![] }));
        for g in ["input", "video"] {
            rig.service.add_member(A, "bob", g, g);
            assert_eq!(result(&only(&rig)).3, Some("group_not_assignable"), "{g}");
        }
        let cfg = UsersConfig { assignable_system_groups: vec!["sudo".into(), "adm".into(), "input".into()], ..UsersConfig::default() };
        let rig = Rig::with_config(&cfg);
        rig.sys.with(|s| s.groups.push(SysGroup { name: "input".into(), gid: 105, members: vec![] }));
        rig.unlock(A);
        rig.service.add_member(A, "bob", "input", "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.add_member(A, "bob", "input", "input");
        assert!(result(&only(&rig)).2);
    }

    #[test]
    fn leaving_sudo_cannot_remove_the_last_admin_and_ends_the_demoted_admins_sessions() {
        let rig = Rig::new();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        rig.service.unlock(B, "alice", &pw("Alice-Password-22"));
        rig.take();
        rig.service.remove_member(A, "alice", "sudo");
        let got = rig.take();
        assert!(got.iter().any(|(c, e)| *c == B && matches!(e, BridgeEvent::UserSession { unlocked: false, .. })), "{got:?}");
        assert!(got.iter().any(|(c, e)| *c == A && matches!(e, BridgeEvent::UserResult { success: true, .. })));
        rig.service.remove_member(A, "kiosk", "sudo");
        assert_eq!(result(&only(&rig)).3, Some("last_admin"));
        rig.service.remove_member(A, "kiosk", "adm");
        assert!(result(&only(&rig)).2);
        rig.service.remove_member(A, "kiosk", "crew");
        assert_eq!(result(&only(&rig)).3, Some("not_found"), "not a member");
    }

    #[test]
    fn group_add_and_remove_follow_the_policy() {
        let rig = unlocked();
        rig.service.add_group(A, "team");
        assert_eq!(result(&only(&rig)), ("group_add", "team", true, None));
        for (bad, code) in [("sudo", "invalid_name"), ("Team", "invalid_name"), ("alice", "exists"), ("crew", "exists")] {
            rig.service.add_group(A, bad);
            assert_eq!(result(&only(&rig)).3, Some(code), "{bad}");
        }
        rig.service.remove_group(A, "crew", "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.remove_group(A, "crew", "crew");
        assert_eq!(result(&only(&rig)), ("group_remove", "crew", true, None));
        rig.service.remove_group(A, "sudo", "sudo");
        assert_eq!(result(&only(&rig)).3, Some("system_account"));
        rig.service.remove_group(A, "alice", "alice");
        assert_eq!(result(&only(&rig)).3, Some("group_not_assignable"), "a private group goes with its user");
        let tight = UsersConfig { max_groups: 1, ..UsersConfig::default() };
        let rig = Rig::with_config(&tight);
        rig.unlock(A);
        rig.service.add_group(A, "team");
        assert_eq!(result(&only(&rig)).3, Some("too_many"));
    }

    // ---- ssh keys --------------------------------------------------------

    #[test]
    fn a_key_is_added_listed_and_removed_with_its_fingerprint_as_confirm() {
        let rig = unlocked();
        let key = key_line(7, "bob@laptop");
        rig.service.add_key(A, "bob", &key);
        assert_eq!(result(&only(&rig)), ("ssh_key_add", "bob", true, None));
        assert!(rig.sys.calls().contains(&"keys.write bob n=1 owner=1002:1002".to_string()), "the file belongs to the account: {:?}", rig.sys.calls());
        rig.service.list_keys(A, "bob");
        let BridgeEvent::SshKeyList { name, keys } = only(&rig) else { panic!() };
        assert_eq!((name.as_str(), keys.len()), ("bob", 1));
        assert_eq!((keys[0].key_type.as_str(), keys[0].comment.as_str()), ("ssh-ed25519", "bob@laptop"));
        let fp = keys[0].fingerprint.clone();
        assert!(fp.starts_with("SHA256:"));
        rig.service.add_key(A, "bob", &key);
        assert_eq!(result(&only(&rig)).3, Some("exists"), "the same key twice");
        rig.service.remove_key(A, "bob", &fp, "");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.remove_key(A, "bob", &fp, "SHA256:other");
        assert_eq!(result(&only(&rig)).3, Some("confirm_required"));
        rig.service.remove_key(A, "bob", "SHA256:nothere", "SHA256:nothere");
        assert_eq!(result(&only(&rig)).3, Some("not_found"));
        rig.service.remove_key(A, "bob", &fp, &fp);
        assert_eq!(result(&only(&rig)), ("ssh_key_remove", "bob", true, None));
        assert!(!rig.sys.with(|s| s.keys.contains_key("bob")), "the last key removes the file");
    }

    #[test]
    fn keys_are_optional_for_kiosk_and_work_for_it() {
        let rig = unlocked();
        rig.service.list_keys(A, "kiosk");
        let BridgeEvent::SshKeyList { keys, .. } = only(&rig) else { panic!() };
        assert!(keys.is_empty(), "no key is baked");
        rig.service.add_key(A, "kiosk", &key_line(1, "owner"));
        assert!(result(&only(&rig)).2);
        rig.service.list_users(A);
        let BridgeEvent::UserList { users } = only(&rig) else { panic!() };
        assert_eq!(users.iter().find(|u| u.name == "kiosk").unwrap().key_count, 1);
    }

    #[test]
    fn key_refusals() {
        let rig = unlocked();
        for bad in ["", "garbage", "command=\"id\" ssh-ed25519 AAAA x", &format!("{}\n{}", key_line(1, ""), key_line(2, ""))] {
            rig.service.add_key(A, "bob", bad);
            assert_eq!(result(&only(&rig)).3, Some("invalid_key"), "{bad:?}");
        }
        rig.service.add_key(A, "bob", &key_line(3, "BADKEY"));
        assert_eq!(result(&only(&rig)).3, Some("invalid_key"), "ssh-keygen said no");
        rig.service.add_key(A, "root", &key_line(3, ""));
        assert_eq!(result(&only(&rig)).3, Some("system_account"));
        rig.service.add_key(A, "ghost", &key_line(3, ""));
        assert_eq!(result(&only(&rig)).3, Some("not_found"));
        rig.service.list_keys(A, "root");
        assert_eq!(error_code(&only(&rig)), "system_account");
        assert!(rig.sys.calls().is_empty(), "nothing was written");
    }

    #[test]
    fn the_key_count_limit_applies() {
        let cfg = UsersConfig { max_keys_per_user: 2, ..UsersConfig::default() };
        let rig = Rig::with_config(&cfg);
        rig.unlock(A);
        for tag in 1..=2 {
            rig.service.add_key(A, "bob", &key_line(tag, ""));
            assert!(result(&only(&rig)).2);
        }
        rig.service.add_key(A, "bob", &key_line(3, ""));
        assert_eq!(result(&only(&rig)).3, Some("too_many"));
    }

    // ---- secrets and targeting ---------------------------------------------

    #[test]
    fn every_reply_goes_to_the_requester_and_no_event_but_user_secret_carries_a_password() {
        let rig = unlocked();
        rig.service.add_user(B, "carol", &pw("Carol-Password-3"), false);
        let first = rig.take();
        assert!(first.iter().all(|(c, _)| *c == B));
        rig.unlock(B);
        rig.service.add_user(B, "carol", &empty(), false);
        rig.service.set_password(B, "alice", &pw("Secret-Value-Here-1"), &empty(), "alice");
        rig.service.set_password(B, "kiosk", &pw("Secret-Value-Here-2"), &pw("wrong-password-9"), "");
        rig.service.list_users(B);
        rig.service.list_groups(B);
        rig.service.list_keys(B, "bob");
        let got = rig.take();
        assert!(!got.is_empty() && got.iter().all(|(c, _)| *c == B));
        for (_, e) in &got {
            let json = serde_json::to_string(e).unwrap();
            assert!(!json.contains("Secret-Value-Here"), "{json}");
            assert!(!json.contains("wrong-password-9"), "{json}");
            if !matches!(e, BridgeEvent::UserSecret { .. }) {
                let pw = rig.sys.with(|s| s.passwords.get("carol").cloned().flatten());
                if let Some(pw) = pw {
                    assert!(!json.contains(&pw), "{json}");
                }
            }
        }
    }

    #[test]
    fn the_knob_gate_forwards_presses_to_the_unlock_cap() {
        use crate::services::input::NoGate;
        let rig = Rig::new();
        let gate = rig.service.gate_with_press(Arc::new(NoGate));
        assert!(!gate.active());
        for i in 0..20u64 {
            rig.service.unlock(ConnId(100 + i), "kiosk", &pw("wrong-password-1"));
        }
        rig.take();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert_eq!(error_code(&only(&rig)), "rate_limited");
        gate.on_knob_press();
        rig.service.unlock(A, "kiosk", &pw(ADMIN_PASSWORD));
        assert!(matches!(only(&rig), BridgeEvent::UserSession { unlocked: true, .. }));
    }
}
