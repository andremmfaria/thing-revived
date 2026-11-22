//! First-boot password service (plan sections 5 and 7, phase 1): status,
//! reveal behind a physical knob push, acknowledgement by proof, and expiry.
//!
//! Every method returns the one event that answers the request. The router
//! sends it to the requesting connection only; nothing here broadcasts, and
//! nothing here formats the password, the proof or a wrong guess into a log
//! line or an error text.
//!
//! The secret lives in the tmpfs holder and in memory (`FirstBoot`). `reveal`
//! is honoured only when the controller's own evdev reader saw the knob push
//! (`KEY_ENTER`) inside the press window: a websocket client cannot create
//! that press, so a local process that merely connects cannot read the secret.

use crate::app::hub::ConnId;
use crate::domain::accounts::state::{BootstrapError, FirstBoot, Phase, KIOSK_USER};
use crate::domain::keys::KEY_ENTER;
use crate::ports::{AccountFiles, AccountStateStore, Clock, EventSink, MonotonicClock, SecretStore};
use crate::protocol::BridgeEvent;
use crate::sync_util::lock;
use crate::utils::config::UsersConfig;
use log::{debug, info, warn};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CTX_GET: &str = "user_bootstrap_get";
const CTX_REVEAL: &str = "user_bootstrap_reveal";
const CTX_ACK: &str = "user_bootstrap_ack";

/// First wait after the free attempts are used up; doubles per failure.
const FIRST_BACKOFF_SECS: u64 = 5;

/// Records the last physical knob push and releases it to at most one reveal.
pub struct PressGate {
    clock: Arc<dyn Clock>,
    window: Duration,
    last: Mutex<Option<Instant>>,
}

impl PressGate {
    pub fn new(clock: Arc<dyn Clock>, window: Duration) -> PressGate {
        PressGate { clock, window, last: Mutex::new(None) }
    }

    /// Called from the evdev reader on a knob push.
    pub fn record(&self) {
        *lock(&self.last) = Some(self.clock.now());
    }

    /// True once per press, and only while the press is inside the window.
    pub fn consume(&self) -> bool {
        match lock(&self.last).take() {
            Some(at) => self.clock.now().saturating_duration_since(at) <= self.window,
            None => false,
        }
    }
}

/// An `EventSink` in front of the real one that notes knob pushes. Wrapped
/// around the sink the button reader emits to, so the press source is the
/// hardware reader and nothing a websocket client can send.
pub struct PressTap {
    inner: Arc<dyn EventSink>,
    gate: Arc<PressGate>,
}

impl PressTap {
    pub fn new(inner: Arc<dyn EventSink>, gate: Arc<PressGate>) -> PressTap {
        PressTap { inner, gate }
    }
}

impl EventSink for PressTap {
    fn emit(&self, e: BridgeEvent) {
        if let BridgeEvent::Button { code: KEY_ENTER, value: 1 } = e {
            self.gate.record();
        }
        self.inner.emit(e);
    }
}

/// Wrong-proof counter with doubling backoff. Global, not per connection: a
/// local process can open as many connections as it likes.
struct AckLimiter {
    failures: u32,
    blocked_until: Option<Instant>,
}

impl AckLimiter {
    fn new() -> AckLimiter {
        AckLimiter { failures: 0, blocked_until: None }
    }

    /// `Err(wait)` while a backoff is running.
    fn check(&self, now: Instant) -> Result<(), Duration> {
        match self.blocked_until {
            Some(until) if now < until => Err(until - now),
            _ => Ok(()),
        }
    }

    fn fail(&mut self, now: Instant, free: u32, max: Duration) {
        self.failures = self.failures.saturating_add(1);
        if self.failures > free {
            let doublings = (self.failures - free - 1).min(20);
            let wait = Duration::from_secs(FIRST_BACKOFF_SECS.saturating_mul(1u64 << doublings)).min(max);
            self.blocked_until = Some(now + wait);
        }
    }

    fn reset(&mut self) {
        self.failures = 0;
        self.blocked_until = None;
    }
}

struct Inner {
    /// `None` until a pending password could be loaded (nothing generated,
    /// or provision has not run).
    boot: Option<FirstBoot>,
    ack: AckLimiter,
    /// The tmpfs holder must be scrubbed (acknowledged or expired).
    scrub_due: bool,
    expiry_seen: bool,
    tick_failing: bool,
}

pub struct UsersPorts {
    pub secrets: Arc<dyn SecretStore>,
    pub state: Arc<dyn AccountStateStore>,
    pub files: Arc<dyn AccountFiles>,
    /// Whole monotonic seconds: the expiry in the tmpfs file is measured on it.
    pub monotonic: Arc<dyn MonotonicClock>,
    /// Instants: press window and backoff.
    pub timer: Arc<dyn Clock>,
}

pub struct UsersService {
    ports: UsersPorts,
    presses: Arc<PressGate>,
    ack_free_attempts: u32,
    ack_backoff_max: Duration,
    inner: Mutex<Inner>,
}

impl UsersService {
    pub fn new(ports: UsersPorts, presses: Arc<PressGate>, cfg: &UsersConfig) -> UsersService {
        UsersService {
            ports,
            presses,
            ack_free_attempts: cfg.ack_free_attempts,
            ack_backoff_max: Duration::from_secs(cfg.ack_backoff_max_secs),
            inner: Mutex::new(Inner { boot: None, ack: AckLimiter::new(), scrub_due: false, expiry_seen: false, tick_failing: false }),
        }
    }

    /// `user_bootstrap_get`: the status, never the secret.
    pub fn bootstrap_get(&self) -> BridgeEvent {
        let mut inner = lock(&self.inner);
        if !self.ports.files.storage_available() {
            return status_event(false, false, None, false);
        }
        if let Err(e) = self.prepare(&mut inner) {
            return e.event(CTX_GET);
        }
        self.status(&mut inner)
    }

    /// `user_bootstrap_reveal`: the secret, for one fresh knob push.
    pub fn bootstrap_reveal(&self, conn: ConnId) -> BridgeEvent {
        let mut inner = lock(&self.inner);
        if !self.ports.files.storage_available() {
            return Failure::StorageUnavailable.event(CTX_REVEAL);
        }
        if let Err(e) = self.prepare(&mut inner) {
            return e.event(CTX_REVEAL);
        }
        if let Err(e) = live_phase(&mut inner, self.ports.monotonic.now_secs()) {
            return e.event(CTX_REVEAL);
        }
        // Checked after the phase so a press is not burned on nothing.
        if !self.presses.consume() {
            debug!("bootstrap: reveal from connection {} refused, no recent knob push", conn.0);
            return Failure::PressRequired.event(CTX_REVEAL);
        }
        let now = self.ports.monotonic.now_secs();
        let Some(boot) = inner.boot.as_mut() else { return Failure::NotPending.event(CTX_REVEAL) };
        let expires_in_secs = boot.expires_in_secs(now).unwrap_or(0);
        match boot.reveal(now) {
            Ok(secret) => {
                info!("bootstrap: the first-boot password was revealed to connection {}", conn.0);
                BridgeEvent::UserBootstrapSecret { username: KIOSK_USER.into(), password: secret.into(), expires_in_secs }
            }
            Err(e) => Failure::from(e).event(CTX_REVEAL),
        }
    }

    /// `user_bootstrap_ack`: proof by the last four characters.
    pub fn bootstrap_ack(&self, conn: ConnId, last4: &str) -> BridgeEvent {
        let mut inner = lock(&self.inner);
        if !self.ports.files.storage_available() {
            return Failure::StorageUnavailable.event(CTX_ACK);
        }
        if let Err(e) = self.prepare(&mut inner) {
            return e.event(CTX_ACK);
        }
        let timer_now = self.ports.timer.now();
        if let Err(wait) = inner.ack.check(timer_now) {
            return Failure::RateLimited(wait.as_secs() + 1).event(CTX_ACK);
        }
        let now = self.ports.monotonic.now_secs();
        let Some(boot) = inner.boot.as_mut() else { return Failure::NotPending.event(CTX_ACK) };
        match boot.acknowledge(now, last4) {
            Ok(()) => match self.record_ack(&mut inner) {
                Ok(()) => {
                    inner.ack.reset();
                    info!("bootstrap: the first-boot password was acknowledged by connection {}", conn.0);
                    self.status(&mut inner)
                }
                Err(e) => e.event(CTX_ACK),
            },
            Err(BootstrapError::BadAck) => {
                inner.ack.fail(timer_now, self.ack_free_attempts, self.ack_backoff_max);
                warn!("bootstrap: wrong acknowledgement from connection {}", conn.0);
                Failure::BadAck.event(CTX_ACK)
            }
            Err(e) => {
                let failure = Failure::from(e);
                self.settle(&mut inner);
                failure.event(CTX_ACK)
            }
        }
    }

    /// Housekeeping for a timer thread: expiry scrubs the holder even when
    /// nobody is talking to the controller.
    pub fn tick(&self) {
        let mut inner = lock(&self.inner);
        let now = self.ports.monotonic.now_secs();
        let finished = matches!(inner.boot.as_mut().map(|b| b.phase(now)), Some(Phase::Acknowledged));
        if (finished && !inner.scrub_due) || !self.ports.files.storage_available() {
            return;
        }
        let result = self.prepare(&mut inner);
        match (&result, inner.tick_failing) {
            (Err(_), false) => warn!("bootstrap: housekeeping failed, will retry"),
            (Ok(()), true) => info!("bootstrap: housekeeping recovered"),
            _ => {}
        }
        inner.tick_failing = result.is_err();
    }

    /// Loads what is pending if nothing is loaded yet, applies expiry and
    /// retries a scrub that failed earlier.
    fn prepare(&self, inner: &mut Inner) -> Result<(), Failure> {
        if inner.boot.is_none() {
            inner.boot = self.load_boot()?;
        }
        self.settle(inner);
        Ok(())
    }

    fn load_boot(&self) -> Result<Option<FirstBoot>, Failure> {
        let state = self.ports.state.load().map_err(|e| Failure::internal("read account state", &e))?;
        let Some(state) = state else { return Ok(None) };
        if state.acknowledged {
            return Ok(Some(FirstBoot::acknowledged()));
        }
        let stored = self.ports.secrets.load().map_err(|e| Failure::internal("read the first-boot secret", &e))?;
        Ok(stored.map(|s| FirstBoot::pending(s.secret, s.expires_at_secs)))
    }

    /// Notes expiry and performs a due scrub.
    fn settle(&self, inner: &mut Inner) {
        let now = self.ports.monotonic.now_secs();
        if let Some(boot) = inner.boot.as_mut() {
            if boot.phase(now) == Phase::Expired && !inner.expiry_seen {
                inner.expiry_seen = true;
                inner.scrub_due = true;
                info!("bootstrap: the first-boot password expired and is being scrubbed");
            }
        }
        self.flush_scrub(inner);
    }

    fn flush_scrub(&self, inner: &mut Inner) {
        if !inner.scrub_due {
            return;
        }
        match self.ports.secrets.scrub() {
            Ok(()) => inner.scrub_due = false,
            Err(e) => warn!("bootstrap: scrubbing the first-boot secret failed: {e}"),
        }
    }

    /// Persists the acknowledgement, then scrubs. The state is written first
    /// so a failure here leaves a password that is still on tmpfs and still
    /// acknowledgeable, never a scrubbed one that `state.json` says is open.
    fn record_ack(&self, inner: &mut Inner) -> Result<(), Failure> {
        let saved = match self.ports.state.load() {
            Ok(Some(s)) if s.acknowledged => Ok(()),
            Ok(Some(s)) => self.ports.state.save(&s.acknowledged()),
            Ok(None) => Err(crate::app::error::AppError::failed("account state is missing")),
            Err(e) => Err(e),
        };
        if let Err(e) = saved {
            // `FirstBoot` already dropped its copy: reload it from tmpfs.
            inner.boot = self.load_boot().ok().flatten();
            return Err(Failure::internal("record the acknowledgement", &e));
        }
        inner.scrub_due = true;
        self.flush_scrub(inner);
        if inner.scrub_due {
            return Err(Failure::Internal("Could not remove the temporary password file".into()));
        }
        Ok(())
    }

    fn status(&self, inner: &mut Inner) -> BridgeEvent {
        let now = self.ports.monotonic.now_secs();
        match inner.boot.as_mut() {
            None => status_event(false, false, None, true),
            Some(boot) => match boot.phase(now) {
                Phase::Pending | Phase::Revealed => status_event(true, false, boot.expires_in_secs(now), true),
                Phase::Acknowledged => status_event(false, true, None, true),
                Phase::Expired => status_event(false, false, None, true),
            },
        }
    }
}

fn status_event(pending: bool, acknowledged: bool, expires_in_secs: Option<u64>, storage_ok: bool) -> BridgeEvent {
    BridgeEvent::UserBootstrap { pending, username: KIOSK_USER.into(), acknowledged, expires_in_secs, storage_ok }
}

/// Ok while a password is waiting; otherwise the reason there is none.
fn live_phase(inner: &mut Inner, now: u64) -> Result<(), Failure> {
    match inner.boot.as_mut().map(|b| b.phase(now)) {
        Some(Phase::Pending) | Some(Phase::Revealed) => Ok(()),
        Some(Phase::Expired) => Err(Failure::Expired),
        Some(Phase::Acknowledged) | None => Err(Failure::NotPending),
    }
}

/// A refusal, mapped to a wire `code` and display text. Messages never
/// include a password, a proof or an inner error that could carry one.
enum Failure {
    StorageUnavailable,
    PressRequired,
    BadAck,
    RateLimited(u64),
    Expired,
    NotPending,
    Internal(String),
}

impl Failure {
    /// Logs the inner error (which never holds a secret: store and port errors
    /// name paths and tools only) and keeps the wire text generic.
    fn internal(what: &str, e: &crate::app::error::AppError) -> Failure {
        warn!("bootstrap: could not {what}: {e}");
        Failure::Internal(format!("Could not {what}"))
    }

    fn event(self, context: &str) -> BridgeEvent {
        let (code, message) = match self {
            Failure::StorageUnavailable => ("storage_unavailable", "Account storage is unavailable".to_string()),
            Failure::PressRequired => ("press_required", "Press the knob, then ask again".to_string()),
            Failure::BadAck => ("bad_ack", "The last four characters do not match".to_string()),
            Failure::RateLimited(secs) => ("rate_limited", format!("Too many wrong attempts, try again in {secs} s")),
            Failure::Expired => ("expired", "The password expired, reboot to generate a new one".to_string()),
            Failure::NotPending => ("not_pending", "No first-boot password is waiting".to_string()),
            Failure::Internal(m) => ("internal", m),
        };
        BridgeEvent::UserError { context: context.into(), code: code.into(), message }
    }
}

impl From<BootstrapError> for Failure {
    fn from(e: BootstrapError) -> Failure {
        match e {
            BootstrapError::BadAck => Failure::BadAck,
            BootstrapError::Expired => Failure::Expired,
            BootstrapError::NotPending => Failure::NotPending,
        }
    }
}

/// Fake ports for tests (also used by `tests/no_secret_logging.rs`).
pub mod testing {
    use super::*;
    use crate::app::error::AppError;
    use crate::domain::accounts::password::Secret;
    use crate::domain::accounts::state::{AccountFile, AccountState};
    use crate::ports::StoredSecret;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Hand-advanced instants.
    pub struct FakeTimer {
        base: Instant,
        offset_ms: AtomicU64,
    }

    impl FakeTimer {
        pub fn new() -> FakeTimer {
            FakeTimer { base: Instant::now(), offset_ms: AtomicU64::new(0) }
        }

        pub fn advance(&self, d: Duration) {
            self.offset_ms.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
        }
    }

    impl Default for FakeTimer {
        fn default() -> Self {
            FakeTimer::new()
        }
    }

    impl Clock for FakeTimer {
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
        }

        fn sleep_until(&self, _deadline: Instant) {}
    }

    /// Monotonic seconds, hand-advanced.
    #[derive(Default)]
    pub struct FakeMonotonic(pub AtomicU64);

    impl MonotonicClock for FakeMonotonic {
        fn now_secs(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[derive(Default)]
    pub struct FakeSecrets {
        pub held: Mutex<Option<(String, u64)>>,
        pub fail_scrub: Mutex<bool>,
    }

    impl SecretStore for FakeSecrets {
        fn store(&self, secret: &Secret, expires_at_secs: u64) -> Result<(), AppError> {
            *lock(&self.held) = Some((secret.expose().to_string(), expires_at_secs));
            Ok(())
        }

        fn load(&self) -> Result<Option<StoredSecret>, AppError> {
            Ok(lock(&self.held).as_ref().map(|(p, e)| StoredSecret { secret: Secret::new(p.clone()), expires_at_secs: *e }))
        }

        fn scrub(&self) -> Result<(), AppError> {
            if *lock(&self.fail_scrub) {
                return Err(AppError::failed("scrub refused"));
            }
            *lock(&self.held) = None;
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct FakeState {
        pub held: Mutex<Option<AccountState>>,
        pub fail_save: Mutex<bool>,
    }

    impl AccountStateStore for FakeState {
        fn load(&self) -> Result<Option<AccountState>, AppError> {
            Ok(lock(&self.held).clone())
        }

        fn save(&self, state: &AccountState) -> Result<(), AppError> {
            if *lock(&self.fail_save) {
                return Err(AppError::failed("disk full"));
            }
            *lock(&self.held) = Some(state.clone());
            Ok(())
        }
    }

    pub struct FakeFiles {
        pub available: Mutex<bool>,
    }

    impl Default for FakeFiles {
        fn default() -> Self {
            FakeFiles { available: Mutex::new(true) }
        }
    }

    impl AccountFiles for FakeFiles {
        fn storage_available(&self) -> bool {
            *lock(&self.available)
        }
        fn read_lower(&self, _: AccountFile) -> Result<String, AppError> {
            Ok(String::new())
        }
        fn read_current(&self, _: AccountFile) -> Result<String, AppError> {
            Ok(String::new())
        }
        fn append_lines(&self, _: AccountFile, _: &[String]) -> Result<(), AppError> {
            Ok(())
        }
        fn list_upper(&self) -> Result<Vec<String>, AppError> {
            Ok(Vec::new())
        }
        fn list_key_files(&self) -> Result<Vec<String>, AppError> {
            Ok(Vec::new())
        }
        fn delete_key_file(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        fn baked_key_present(&self) -> bool {
            true
        }
    }

    /// A service over fakes with one pending password, as `provision` leaves
    /// it: tmpfs holder written, `state.json` pending.
    pub struct Rig {
        pub service: Arc<UsersService>,
        pub secrets: Arc<FakeSecrets>,
        pub state: Arc<FakeState>,
        pub files: Arc<FakeFiles>,
        pub mono: Arc<FakeMonotonic>,
        pub timer: Arc<FakeTimer>,
        pub presses: Arc<PressGate>,
    }

    pub const TTL_SECS: u64 = 900;

    impl Rig {
        pub fn pending(password: &str) -> Rig {
            let rig = Rig::empty();
            *lock(&rig.secrets.held) = Some((password.to_string(), TTL_SECS));
            *lock(&rig.state.held) = Some(AccountState::pending("e".repeat(32)));
            rig
        }

        pub fn empty() -> Rig {
            let secrets = Arc::new(FakeSecrets::default());
            let state = Arc::new(FakeState::default());
            let files = Arc::new(FakeFiles::default());
            let mono = Arc::new(FakeMonotonic::default());
            let timer = Arc::new(FakeTimer::new());
            let cfg = UsersConfig::default();
            let presses = Arc::new(PressGate::new(timer.clone(), Duration::from_secs(cfg.reveal_press_window_secs)));
            let ports = UsersPorts {
                secrets: secrets.clone(),
                state: state.clone(),
                files: files.clone(),
                monotonic: mono.clone(),
                timer: timer.clone(),
            };
            let service = Arc::new(UsersService::new(ports, presses.clone(), &cfg));
            Rig { service, secrets, state, files, mono, timer, presses }
        }

        pub fn set_now(&self, secs: u64) {
            self.mono.0.store(secs, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::domain::accounts::state::AccountState;

    const PW: &str = "Zk7mQp3Rw9XcT2vB";
    const CONN: ConnId = ConnId(3);

    fn secret_of(e: &BridgeEvent) -> Option<String> {
        match e {
            BridgeEvent::UserBootstrapSecret { password, .. } => Some(password.expose().to_string()),
            _ => None,
        }
    }

    fn code_of(e: &BridgeEvent) -> &str {
        match e {
            BridgeEvent::UserError { code, .. } => code,
            other => panic!("not an error: {other:?}"),
        }
    }

    fn held(rig: &Rig) -> bool {
        lock(&rig.secrets.held).is_some()
    }

    #[test]
    fn status_reports_pending_without_the_secret() {
        let rig = Rig::pending(PW);
        rig.set_now(100);
        match rig.service.bootstrap_get() {
            BridgeEvent::UserBootstrap { pending: true, username, acknowledged: false, expires_in_secs: Some(800), storage_ok: true } => {
                assert_eq!(username, "kiosk");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reveal_needs_a_knob_push_and_one_push_gives_one_reveal() {
        let rig = Rig::pending(PW);
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "press_required");
        rig.presses.record();
        let first = rig.service.bootstrap_reveal(CONN);
        assert_eq!(secret_of(&first).as_deref(), Some(PW));
        assert!(matches!(first, BridgeEvent::UserBootstrapSecret { expires_in_secs: 900, .. }));
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "press_required", "the push was consumed");
        rig.presses.record();
        assert!(secret_of(&rig.service.bootstrap_reveal(ConnId(4))).is_some(), "a new push reveals again (Chromium crash)");
    }

    #[test]
    fn a_push_older_than_the_window_does_not_reveal() {
        let rig = Rig::pending(PW);
        rig.presses.record();
        rig.timer.advance(Duration::from_secs(10));
        assert!(secret_of(&rig.service.bootstrap_reveal(CONN)).is_some(), "exactly at the window is still valid");
        rig.presses.record();
        rig.timer.advance(Duration::from_millis(10_001));
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "press_required");
    }

    #[test]
    fn only_a_knob_push_edge_on_the_hardware_sink_counts_as_a_press() {
        use crate::app::hub::RecordingSink;
        let rig = Rig::pending(PW);
        let hub = Arc::new(RecordingSink::new());
        let tap = PressTap::new(hub.clone(), rig.presses.clone());
        for (code, value) in [(KEY_ENTER, 0), (KEY_ENTER, 2), (50, 1), (1, 1), (2, 1)] {
            tap.emit(BridgeEvent::Button { code, value });
        }
        tap.emit(BridgeEvent::Knob { delta: 1 });
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "press_required");
        assert_eq!(hub.take().len(), 6, "every event is still forwarded");
        tap.emit(BridgeEvent::Button { code: KEY_ENTER, value: 1 });
        assert!(secret_of(&rig.service.bootstrap_reveal(CONN)).is_some());
    }

    #[test]
    fn a_refused_reveal_does_not_burn_the_press() {
        let rig = Rig::empty();
        *lock(&rig.state.held) = Some(AccountState::pending("e".repeat(32)).acknowledged());
        rig.presses.record();
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "not_pending");
        assert!(rig.presses.consume(), "nothing was revealed, so the press is still there");
    }

    #[test]
    fn ack_with_the_right_last4_scrubs_and_records_the_state() {
        let rig = Rig::pending(PW);
        let ev = rig.service.bootstrap_ack(CONN, "vB");
        assert_eq!(code_of(&ev), "bad_ack", "too short");
        let ev = rig.service.bootstrap_ack(CONN, &PW[PW.len() - 4..]);
        assert!(matches!(ev, BridgeEvent::UserBootstrap { pending: false, acknowledged: true, expires_in_secs: None, .. }), "{ev:?}");
        assert!(!held(&rig), "tmpfs holder removed");
        assert!(lock(&rig.state.held).as_ref().unwrap().acknowledged);
        let after = rig.service.bootstrap_reveal(CONN);
        assert_eq!(code_of(&after), "not_pending");
        // Acknowledging again is harmless.
        assert!(matches!(rig.service.bootstrap_ack(CONN, "anyx"), BridgeEvent::UserBootstrap { acknowledged: true, .. }));
    }

    #[test]
    fn a_wrong_ack_keeps_the_secret_and_state() {
        let rig = Rig::pending(PW);
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, "zzzz")), "bad_ack");
        assert!(held(&rig));
        assert!(!lock(&rig.state.held).as_ref().unwrap().acknowledged);
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: true, .. }));
    }

    #[test]
    fn wrong_acks_back_off_after_the_free_attempts_and_even_the_right_value_waits() {
        let rig = Rig::pending(PW);
        let right = &PW[PW.len() - 4..];
        for _ in 0..3 {
            assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, "zzzz")), "bad_ack");
        }
        // 4th failure starts a 5 s backoff.
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, "zzzz")), "bad_ack");
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, right)), "rate_limited", "no oracle during backoff");
        assert_eq!(code_of(&rig.service.bootstrap_ack(ConnId(99), right)), "rate_limited", "reconnecting does not help");
        rig.timer.advance(Duration::from_secs(5));
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, "zzzz")), "bad_ack");
        // Second backoff is 10 s.
        rig.timer.advance(Duration::from_secs(9));
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, right)), "rate_limited");
        rig.timer.advance(Duration::from_secs(1));
        assert!(matches!(rig.service.bootstrap_ack(CONN, right), BridgeEvent::UserBootstrap { acknowledged: true, .. }));
    }

    #[test]
    fn backoff_is_capped_at_the_configured_maximum() {
        let mut l = AckLimiter::new();
        let t0 = Instant::now();
        for _ in 0..40 {
            l.fail(t0, 3, Duration::from_secs(300));
        }
        assert_eq!(l.check(t0).unwrap_err(), Duration::from_secs(300));
    }

    #[test]
    fn expiry_scrubs_without_any_request() {
        let rig = Rig::pending(PW);
        rig.set_now(899);
        rig.service.tick();
        assert!(held(&rig));
        rig.set_now(900);
        rig.service.tick();
        assert!(!held(&rig), "the timer thread scrubs at the deadline");
        assert!(lock(&rig.state.held).as_ref().is_some_and(|s| !s.acknowledged), "state is untouched: a reboot regenerates");
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: false, acknowledged: false, expires_in_secs: None, .. }));
        rig.presses.record();
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "expired");
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, &PW[PW.len() - 4..])), "expired");
    }

    #[test]
    fn expiry_noticed_by_a_request_also_scrubs() {
        let rig = Rig::pending(PW);
        rig.set_now(5000);
        rig.presses.record();
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "expired");
        assert!(!held(&rig));
    }

    #[test]
    fn a_failed_scrub_is_retried_on_the_next_tick() {
        let rig = Rig::pending(PW);
        *lock(&rig.secrets.fail_scrub) = true;
        let ev = rig.service.bootstrap_ack(CONN, &PW[PW.len() - 4..]);
        assert_eq!(code_of(&ev), "internal");
        assert!(held(&rig));
        *lock(&rig.secrets.fail_scrub) = false;
        rig.service.tick();
        assert!(!held(&rig), "retried");
    }

    #[test]
    fn a_failed_state_write_keeps_the_password_acknowledgeable() {
        let rig = Rig::pending(PW);
        *lock(&rig.state.fail_save) = true;
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, &PW[PW.len() - 4..])), "internal");
        assert!(held(&rig), "not scrubbed while the state says unacknowledged");
        *lock(&rig.state.fail_save) = false;
        assert!(matches!(rig.service.bootstrap_ack(CONN, &PW[PW.len() - 4..]), BridgeEvent::UserBootstrap { acknowledged: true, .. }));
        assert!(!held(&rig));
    }

    #[test]
    fn degraded_storage_reports_storage_not_ok_and_refuses_the_rest() {
        let rig = Rig::pending(PW);
        *lock(&rig.files.available) = false;
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: false, storage_ok: false, .. }));
        rig.presses.record();
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "storage_unavailable");
        assert_eq!(code_of(&rig.service.bootstrap_ack(CONN, "abcd")), "storage_unavailable");
    }

    #[test]
    fn nothing_generated_yet_is_not_pending_and_a_late_provision_is_picked_up() {
        let rig = Rig::empty();
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: false, acknowledged: false, storage_ok: true, .. }));
        *lock(&rig.secrets.held) = Some((PW.into(), TTL_SECS));
        *lock(&rig.state.held) = Some(AccountState::pending("e".repeat(32)));
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: true, .. }));
    }

    #[test]
    fn already_acknowledged_at_start_has_nothing_to_reveal() {
        let rig = Rig::empty();
        *lock(&rig.state.held) = Some(AccountState::pending("e".repeat(32)).acknowledged());
        assert!(matches!(rig.service.bootstrap_get(), BridgeEvent::UserBootstrap { pending: false, acknowledged: true, .. }));
        rig.presses.record();
        assert_eq!(code_of(&rig.service.bootstrap_reveal(CONN)), "not_pending");
    }
}
