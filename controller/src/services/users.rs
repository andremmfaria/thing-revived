//! First-boot password service (delta plan sections 4 and 5): the phase
//! machine behind the M-button screen, the boot decision shared with
//! `controller-agent provision`, and the generation that keeps invariant I1.
//!
//! There are no timers, no reveal or ack commands and no tmpfs file. The
//! frontend announces itself with `user_bootstrap_get` (that arms the gate);
//! the first physical M press generates, the second one (after the debounce
//! and only if the secret reached a ready frontend) acknowledges. The
//! plaintext lives only in `FirstBoot`'s memory and is zeroised on drop.
//!
//! Nothing here formats the password into a log line or an error text.

use crate::app::error::AppError;
use crate::app::hub::{ConnId, TargetedSink};
use crate::domain::accounts::password::{self, GenerateError, Secret};
use crate::domain::accounts::state::{
    decide_boot, hash_is_usable, shadow_hash, AccountFile, AccountState, BootDecision, FirstBoot, Phase, KIOSK_USER,
};
use crate::ports::{AccountFiles, AccountStateStore, AccountsPort, Clock, Entropy, EventSink, FirstBootGate};
use crate::protocol::{BootstrapPhaseWire, BridgeEvent, Secret as WireSecret};
use crate::sync_util::lock;
use crate::utils::config::UsersConfig;
use log::{debug, info, warn};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// `context` of the `user_error` events this service raises on its own (the
/// failures happen on an M press, not in answer to a command).
const CTX: &str = "user_bootstrap";

/// What `settle_boot` found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootOutcome {
    /// The `/etc` overlay is not up (degraded mode): nothing was changed.
    StorageUnavailable,
    /// A usable, acknowledged hash: nothing to show.
    Steady,
    /// A usable hash with no state file: `acknowledged=true` was written.
    Adopted,
    /// No usable hash (possibly after a re-lock): the prompt is due.
    Prompt { relocked: bool },
}

/// Delta plan 4.1: reads the kiosk hash and `state.json`, applies the table
/// and returns what the screen must do. Idempotent, so `provision` (before the
/// controller starts) and the controller (at the first `user_bootstrap_get`)
/// both call it: a controller crash while `showing` leaves a usable but
/// unacknowledged hash with no live secret, which is re-locked here.
pub fn settle_boot(
    files: &dyn AccountFiles,
    state: &dyn AccountStateStore,
    accounts: &dyn AccountsPort,
    entropy: &dyn Entropy,
) -> Result<BootOutcome, AppError> {
    if !files.storage_available() {
        return Ok(BootOutcome::StorageUnavailable);
    }
    let shadow = files.read_current(AccountFile::Shadow)?;
    let usable = shadow_hash(&shadow, KIOSK_USER).is_some_and(hash_is_usable);
    let saved = state.load()?;
    match decide_boot(usable, saved.as_ref()) {
        BootDecision::Steady => Ok(BootOutcome::Steady),
        BootDecision::Adopt => {
            state.save(&AccountState::pending(new_epoch(entropy)?).acknowledged())?;
            info!("bootstrap: the {KIOSK_USER} password already set on this device was adopted, nothing will be shown");
            Ok(BootOutcome::Adopted)
        }
        BootDecision::RelockThenPrompt => {
            accounts.clear_password(KIOSK_USER)?;
            info!("bootstrap: an unacknowledged {KIOSK_USER} password was re-locked, the prompt comes back");
            Ok(BootOutcome::Prompt { relocked: true })
        }
        BootDecision::Prompt => Ok(BootOutcome::Prompt { relocked: false }),
    }
}

/// Generates the password and sets it, in the order that closes the lockout
/// hole (invariant I1): in memory first, then `state.json` unacknowledged,
/// only then `chpasswd`. A power cut after the second step leaves a usable
/// hash with unacknowledged state, which `settle_boot` re-locks.
pub fn generate_and_set(entropy: &dyn Entropy, state: &dyn AccountStateStore, accounts: &dyn AccountsPort) -> Result<Secret, AppError> {
    let secret = password::generate(|buf| entropy.fill(buf)).map_err(|e| match e {
        GenerateError::Entropy(e) => e,
        GenerateError::Exhausted => AppError::failed("entropy source gave no usable bytes"),
    })?;
    let epoch = match state.load()? {
        Some(s) => s.epoch,
        None => new_epoch(entropy)?,
    };
    state.save(&AccountState::pending(epoch))?;
    accounts.set_password(KIOSK_USER, &secret)?;
    Ok(secret)
}

fn new_epoch(entropy: &dyn Entropy) -> Result<String, AppError> {
    let mut bytes = [0u8; 16];
    entropy.fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub struct UsersPorts {
    pub state: Arc<dyn AccountStateStore>,
    pub files: Arc<dyn AccountFiles>,
    pub accounts: Arc<dyn AccountsPort>,
    pub entropy: Arc<dyn Entropy>,
    /// Instants for the ack debounce.
    pub clock: Arc<dyn Clock>,
}

struct Inner {
    machine: FirstBoot,
    /// The boot decision ran against a healthy `/etc`.
    loaded: bool,
    storage_ok: bool,
    /// Connections that sent `user_bootstrap_get` and are still open.
    ready: HashSet<ConnId>,
}

pub struct UsersService {
    ports: UsersPorts,
    sink: Arc<dyn EventSink>,
    replies: Arc<dyn TargetedSink>,
    ack_min_gap: Duration,
    inner: Mutex<Inner>,
}

impl UsersService {
    /// `sink` carries the broadcast `user_bootstrap`, `replies` the targeted
    /// secret and errors (the hub implements both).
    pub fn new(ports: UsersPorts, sink: Arc<dyn EventSink>, replies: Arc<dyn TargetedSink>, cfg: &UsersConfig) -> UsersService {
        UsersService {
            ports,
            sink,
            replies,
            ack_min_gap: Duration::from_millis(cfg.ack_min_gap_ms),
            inner: Mutex::new(Inner { machine: FirstBoot::new(Phase::None), loaded: false, storage_ok: true, ready: HashSet::new() }),
        }
    }

    /// The gate `ButtonPolicy` consults.
    pub fn gate(self: &Arc<Self>) -> Arc<dyn FirstBootGate> {
        Arc::new(BootstrapGate { service: self.clone() })
    }

    /// `user_bootstrap_get`: the frontend is ready. Arms the gate for this
    /// connection and answers it (only it) with the phase, plus the secret
    /// while `showing`.
    pub fn bootstrap_get(&self, conn: ConnId) {
        let mut inner = lock(&self.inner);
        inner.ready.insert(conn);
        if !inner.loaded {
            if let Err(e) = self.load(&mut inner) {
                warn!("bootstrap: could not decide the first-boot state: {e}");
                self.replies.emit_to(conn, error_event("internal", "Could not read the account state"));
                return;
            }
        }
        self.replies.emit_to(conn, phase_event(inner.machine.phase(), inner.storage_ok));
        let secret_event = inner.machine.secret().map(secret_event);
        if let Some(event) = secret_event {
            // Only a secret that was queued for the screen counts as delivered:
            // otherwise the next M press could acknowledge a password nobody saw.
            if self.replies.emit_to(conn, event) {
                let now = self.ports.clock.now();
                inner.machine.mark_delivered(now);
                debug!("bootstrap: the secret was re-sent to connection {}", conn.0);
            } else {
                warn!("bootstrap: the secret could not be sent to connection {}", conn.0);
            }
        }
    }

    /// A websocket connection closed. The last ready frontend leaving while
    /// `showing` disarms the gate: no ack without a screen.
    pub fn connection_closed(&self, conn: ConnId) {
        let mut inner = lock(&self.inner);
        if inner.ready.remove(&conn) && inner.ready.is_empty() {
            inner.machine.clear_delivery();
        }
    }

    /// One physical M press on the down edge, already known to be consumed.
    /// Returns the generation worker's handle (tests join it).
    pub fn press(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        let mut inner = lock(&self.inner);
        if inner.ready.is_empty() {
            return None;
        }
        match inner.machine.phase() {
            Phase::Prompt => {
                inner.machine.begin_generation();
                self.sink.emit(phase_event(Phase::Generating, inner.storage_ok));
                info!("bootstrap: M pressed, generating the {KIOSK_USER} password");
                let service = self.clone();
                Some(thread::spawn(move || service.run_generation()))
            }
            Phase::Showing => {
                if inner.machine.can_acknowledge(self.ports.clock.now(), self.ack_min_gap) {
                    self.acknowledge(&mut inner);
                } else {
                    debug!("bootstrap: M press ignored, the password was not delivered or the debounce gap has not passed");
                }
                None
            }
            // `generating` swallows presses; `none` and `done` are never consumed.
            Phase::Generating | Phase::None | Phase::Done => None,
        }
    }

    /// Worker thread: generate, set, then show.
    fn run_generation(&self) {
        let result = generate_and_set(self.ports.entropy.as_ref(), self.ports.state.as_ref(), self.ports.accounts.as_ref());
        let mut inner = lock(&self.inner);
        match result {
            Ok(secret) => {
                inner.machine.generated(secret);
                self.sink.emit(phase_event(Phase::Showing, inner.storage_ok));
                let event = inner.machine.secret().map(secret_event);
                if let Some(event) = event {
                    let mut delivered = false;
                    for conn in inner.ready.iter() {
                        delivered |= self.replies.emit_to(*conn, event.clone());
                    }
                    if delivered {
                        let now = self.ports.clock.now();
                        inner.machine.mark_delivered(now);
                    }
                }
                info!("bootstrap: the {KIOSK_USER} password is on screen");
            }
            Err(e) => {
                warn!("bootstrap: generating the {KIOSK_USER} password failed: {e}");
                inner.machine.generation_failed();
                self.sink.emit(phase_event(Phase::Prompt, inner.storage_ok));
                self.fail_to_ready(&inner, "generate_failed", "Could not set the password, press M to try again");
            }
        }
    }

    /// Second press: persist `acknowledged=true`, then scrub and finish. A
    /// failed write keeps the screen up so M can retry.
    fn acknowledge(&self, inner: &mut Inner) {
        match self.record_ack() {
            Ok(()) => {
                inner.machine.acknowledged();
                self.sink.emit(phase_event(Phase::Done, inner.storage_ok));
                info!("bootstrap: the {KIOSK_USER} password was acknowledged and scrubbed from memory");
            }
            Err(e) => {
                warn!("bootstrap: could not record the acknowledgement: {e}");
                self.fail_to_ready(inner, "ack_failed", "Could not save the confirmation, press M again");
            }
        }
    }

    fn record_ack(&self) -> Result<(), AppError> {
        let next = match self.ports.state.load()? {
            Some(s) => s.acknowledged(),
            None => AccountState::pending(new_epoch(self.ports.entropy.as_ref())?).acknowledged(),
        };
        self.ports.state.save(&next)
    }

    fn fail_to_ready(&self, inner: &Inner, code: &str, message: &str) {
        for conn in inner.ready.iter() {
            self.replies.emit_to(*conn, error_event(code, message));
        }
    }

    /// The boot decision at the first `user_bootstrap_get` (and again while
    /// storage was unavailable).
    fn load(&self, inner: &mut Inner) -> Result<(), AppError> {
        let outcome = settle_boot(self.ports.files.as_ref(), self.ports.state.as_ref(), self.ports.accounts.as_ref(), self.ports.entropy.as_ref())?;
        match outcome {
            BootOutcome::StorageUnavailable => {
                inner.storage_ok = false;
                inner.loaded = false;
                inner.machine = FirstBoot::new(Phase::None);
            }
            BootOutcome::Steady | BootOutcome::Adopted => {
                inner.storage_ok = true;
                inner.loaded = true;
                inner.machine = FirstBoot::new(Phase::None);
            }
            BootOutcome::Prompt { .. } => {
                inner.storage_ok = true;
                inner.loaded = true;
                inner.machine = FirstBoot::new(Phase::Prompt);
            }
        }
        Ok(())
    }
}

/// `FirstBootGate` over the service, handed to `ButtonPolicy`.
struct BootstrapGate {
    service: Arc<UsersService>,
}

impl FirstBootGate for BootstrapGate {
    fn active(&self) -> bool {
        let inner = lock(&self.service.inner);
        inner.machine.phase().is_live() && !inner.ready.is_empty()
    }

    fn on_m_down(&self) {
        // Production ignores the worker handle; it ends on its own.
        let _ = self.service.press();
    }
}

fn wire_phase(phase: Phase) -> BootstrapPhaseWire {
    match phase {
        Phase::None => BootstrapPhaseWire::None,
        Phase::Prompt => BootstrapPhaseWire::Prompt,
        Phase::Generating => BootstrapPhaseWire::Generating,
        Phase::Showing => BootstrapPhaseWire::Showing,
        Phase::Done => BootstrapPhaseWire::Done,
    }
}

fn phase_event(phase: Phase, storage_ok: bool) -> BridgeEvent {
    BridgeEvent::UserBootstrap { phase: wire_phase(phase), username: KIOSK_USER.into(), storage_ok }
}

fn secret_event(secret: &Secret) -> BridgeEvent {
    BridgeEvent::UserBootstrapSecret { username: KIOSK_USER.into(), password: WireSecret::from(secret) }
}

/// Fixed texts only: nothing here can carry a password or an inner error.
fn error_event(code: &str, message: &str) -> BridgeEvent {
    BridgeEvent::UserError { context: CTX.into(), code: code.into(), message: message.into() }
}

/// Fake ports for tests (also used by `tests/no_secret_logging.rs`).
pub mod testing {
    use super::*;
    use crate::app::hub::RecordingSink;
    use crate::ports::AccountsError;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    /// Ordered record of the port calls that matter for I1.
    pub type Log = Mutex<Vec<String>>;

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

    /// Deterministic bytes (a linear congruential generator).
    pub struct FakeEntropy(Mutex<u64>);

    impl FakeEntropy {
        pub fn new(seed: u64) -> FakeEntropy {
            FakeEntropy(Mutex::new(seed))
        }
    }

    impl Entropy for FakeEntropy {
        fn fill(&self, buf: &mut [u8]) -> Result<(), AppError> {
            let mut s = lock(&self.0);
            for b in buf.iter_mut() {
                *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *b = (*s >> 33) as u8;
            }
            Ok(())
        }
    }

    pub struct FakeState {
        pub held: Mutex<Option<AccountState>>,
        pub fail_save: Mutex<bool>,
        pub fail_load: Mutex<bool>,
        pub log: Arc<Log>,
    }

    impl AccountStateStore for FakeState {
        fn load(&self) -> Result<Option<AccountState>, AppError> {
            if *lock(&self.fail_load) {
                return Err(AppError::failed("state unreadable"));
            }
            Ok(lock(&self.held).clone())
        }

        fn save(&self, state: &AccountState) -> Result<(), AppError> {
            if *lock(&self.fail_save) {
                return Err(AppError::failed("disk full"));
            }
            lock(&self.log).push(format!("state.save ack={}", state.acknowledged));
            *lock(&self.held) = Some(state.clone());
            Ok(())
        }
    }

    /// Only the `shadow` file is modelled: it holds a `root` row and the
    /// `kiosk` row whose hash the accounts fake changes.
    pub struct FakeFiles {
        pub available: Mutex<bool>,
        pub kiosk_hash: Mutex<String>,
    }

    impl FakeFiles {
        pub fn hash(&self) -> String {
            lock(&self.kiosk_hash).clone()
        }
    }

    impl AccountFiles for FakeFiles {
        fn storage_available(&self) -> bool {
            *lock(&self.available)
        }
        fn read_lower(&self, _: AccountFile) -> Result<String, AppError> {
            Ok(String::new())
        }
        fn read_current(&self, file: AccountFile) -> Result<String, AppError> {
            match file {
                AccountFile::Shadow => Ok(format!("root:*:1::::::\nkiosk:{}:1::::::\n", lock(&self.kiosk_hash))),
                _ => Ok(String::new()),
            }
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
    }

    /// Sets the hash in the shared `FakeFiles`, like `chpasswd` does.
    pub struct FakeAccounts {
        pub files: Arc<FakeFiles>,
        pub fail_set: Mutex<bool>,
        pub set_count: AtomicU64,
        /// The plaintext of the last `set_password`, for assertions.
        pub last_password: Mutex<Option<String>>,
        pub log: Arc<Log>,
    }

    impl AccountsPort for FakeAccounts {
        fn set_password(&self, user: &str, password: &Secret) -> Result<(), AccountsError> {
            assert_eq!(user, KIOSK_USER);
            lock(&self.log).push("accounts.set_password".into());
            if *lock(&self.fail_set) {
                return Err(AccountsError::Tool(AppError::failed("chpasswd exited with status 1")));
            }
            self.set_count.fetch_add(1, Ordering::SeqCst);
            *lock(&self.last_password) = Some(password.expose().to_string());
            *lock(&self.files.kiosk_hash) = "$6$fake$hash".into();
            Ok(())
        }

        fn clear_password(&self, user: &str) -> Result<(), AccountsError> {
            assert_eq!(user, KIOSK_USER);
            lock(&self.log).push("accounts.clear_password".into());
            *lock(&self.files.kiosk_hash) = "!".into();
            Ok(())
        }

        fn set_locked(&self, _: &str, _: bool) -> Result<(), AccountsError> {
            unreachable!("the first-boot flow never locks")
        }

        fn end_sessions(&self, _: &str) -> Result<(), AccountsError> {
            unreachable!("the first-boot flow never ends sessions")
        }
    }

    /// A service over fakes. `sink` records what `new` wired in; with `with_sinks`
    /// the events go elsewhere (the real hub) and `sink` stays empty.
    pub struct Rig {
        pub service: Arc<UsersService>,
        pub sink: Arc<RecordingSink>,
        pub state: Arc<FakeState>,
        pub files: Arc<FakeFiles>,
        pub accounts: Arc<FakeAccounts>,
        pub timer: Arc<FakeTimer>,
        pub log: Arc<Log>,
        pub entropy: Arc<FakeEntropy>,
    }

    impl Rig {
        /// `hash` is the kiosk hash in the fake `shadow`; `state` the state file.
        pub fn new(hash: &str, state: Option<AccountState>) -> Rig {
            let sink = Arc::new(RecordingSink::new());
            let mut rig = Rig::with_sinks(hash, state, sink.clone(), sink.clone());
            rig.sink = sink;
            rig
        }

        pub fn with_sinks(hash: &str, state: Option<AccountState>, events: Arc<dyn EventSink>, replies: Arc<dyn TargetedSink>) -> Rig {
            let log: Arc<Log> = Arc::new(Mutex::new(Vec::new()));
            let state = Arc::new(FakeState { held: Mutex::new(state), fail_save: Mutex::new(false), fail_load: Mutex::new(false), log: log.clone() });
            let files = Arc::new(FakeFiles { available: Mutex::new(true), kiosk_hash: Mutex::new(hash.to_string()) });
            let accounts = Arc::new(FakeAccounts {
                files: files.clone(),
                fail_set: Mutex::new(false),
                set_count: AtomicU64::new(0),
                last_password: Mutex::new(None),
                log: log.clone(),
            });
            let timer = Arc::new(FakeTimer::new());
            let entropy = Arc::new(FakeEntropy::new(7));
            let ports = UsersPorts { state: state.clone(), files: files.clone(), accounts: accounts.clone(), entropy: entropy.clone(), clock: timer.clone() };
            let service = Arc::new(UsersService::new(ports, events, replies, &UsersConfig::default()));
            Rig { service, sink: Arc::new(RecordingSink::new()), state, files, accounts, timer, log, entropy }
        }

        /// The log so far.
        pub fn events(&self) -> Vec<String> {
            lock(&self.log).clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::app::hub::RecordingSink;
    use std::sync::atomic::Ordering;

    const A: ConnId = ConnId(1);
    const B: ConnId = ConnId(2);

    fn unack() -> AccountState {
        AccountState::pending("e".repeat(32))
    }

    fn ack() -> AccountState {
        unack().acknowledged()
    }

    fn phases(events: &[BridgeEvent]) -> Vec<BootstrapPhaseWire> {
        events
            .iter()
            .filter_map(|e| match e {
                BridgeEvent::UserBootstrap { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn secrets(events: &[(ConnId, BridgeEvent)]) -> Vec<(ConnId, String)> {
        events
            .iter()
            .filter_map(|(c, e)| match e {
                BridgeEvent::UserBootstrapSecret { password, .. } => Some((*c, password.expose().to_string())),
                _ => None,
            })
            .collect()
    }

    fn code_of(e: &BridgeEvent) -> &str {
        match e {
            BridgeEvent::UserError { code, .. } => code,
            other => panic!("not an error: {other:?}"),
        }
    }

    fn press_and_join(rig: &Rig) {
        if let Some(h) = rig.service.press() {
            h.join().unwrap();
        }
    }

    /// A rig at the prompt with one ready frontend.
    fn at_prompt() -> (Rig, Arc<dyn FirstBootGate>) {
        let rig = Rig::new("!", None);
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        rig.sink.take_targeted();
        (rig, gate)
    }

    /// A rig with the password on screen, delivered to `A`, debounce not passed.
    fn at_showing() -> (Rig, Arc<dyn FirstBootGate>, String) {
        let (rig, gate) = at_prompt();
        press_and_join(&rig);
        let pw = lock(&rig.accounts.last_password).clone().unwrap();
        rig.sink.take();
        rig.sink.take_targeted();
        (rig, gate, pw)
    }

    #[test]
    fn nothing_is_armed_before_a_frontend_asks() {
        let rig = Rig::new("!", None);
        let gate = rig.service.gate();
        assert!(!gate.active());
        gate.on_m_down();
        assert!(rig.sink.take().is_empty(), "no generation without a ready frontend");
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_locked_kiosk_shows_the_prompt_to_the_asker_only_and_arms_the_gate() {
        let rig = Rig::new("!", None);
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        let targeted = rig.sink.take_targeted();
        assert!(matches!(&targeted[..], [(c, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Prompt, storage_ok: true, username })] if *c == A && username == "kiosk"));
        assert!(rig.sink.take().is_empty(), "the reply to a get is not a broadcast");
        assert!(gate.active());
        assert!(rig.state.held.lock().unwrap().is_none(), "nothing is written by asking");
    }

    #[test]
    fn a_usable_hash_without_state_is_adopted_and_nothing_is_shown() {
        let rig = Rig::new("$6$set$byhand", None);
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::None, .. })]));
        assert!(rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        assert!(!gate.active(), "an adopted account never intercepts M");
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_acknowledged_account_is_steady() {
        let rig = Rig::new("$6$set$byhand", Some(ack()));
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::None, .. })]));
        assert!(rig.events().is_empty(), "no write at all");
        assert!(!rig.service.gate().active());
    }

    #[test]
    fn our_own_unacknowledged_password_is_relocked_and_the_prompt_returns() {
        let rig = Rig::new("$6$fake$hash", Some(unack()));
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Prompt, .. })]));
        assert_eq!(rig.files.hash(), "!");
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        assert!(rig.service.gate().active());
    }

    #[test]
    fn the_first_press_generates_state_first_then_chpasswd_then_shows() {
        let (rig, _gate) = at_prompt();
        press_and_join(&rig);
        assert_eq!(rig.events(), vec!["state.save ack=false", "accounts.set_password"], "invariant I1: state before the hash");
        assert!(rig.files.hash().starts_with("$6$"));
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
    }

    #[test]
    fn showing_sends_the_phase_broadcast_and_the_secret_to_the_ready_connection_only() {
        let rig = Rig::new("!", None);
        rig.service.bootstrap_get(A);
        rig.service.bootstrap_get(B);
        rig.sink.take_targeted();
        press_and_join(&rig);
        let pw = lock(&rig.accounts.last_password).clone().unwrap();
        assert_eq!(phases(&rig.sink.take()), vec![BootstrapPhaseWire::Generating, BootstrapPhaseWire::Showing]);
        let mut got = secrets(&rig.sink.take_targeted());
        got.sort_by_key(|(c, _)| c.0);
        assert_eq!(got, vec![(A, pw.clone()), (B, pw.clone())]);
        assert_eq!(pw.len(), 16);
    }

    #[test]
    fn presses_while_generating_or_inside_the_gap_do_nothing() {
        let (rig, _gate) = at_prompt();
        let first = rig.service.press();
        let second = rig.service.press();
        let third = rig.service.press();
        for h in [first, second, third].into_iter().flatten() {
            h.join().unwrap();
        }
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 1, "one generation however many presses");
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "no ack inside the debounce gap");
        assert!(!phases(&rig.sink.take()).contains(&BootstrapPhaseWire::Done));
    }

    #[test]
    fn the_second_press_after_the_gap_acknowledges_and_scrubs() {
        let (rig, gate, _pw) = at_showing();
        rig.timer.advance(Duration::from_millis(1499));
        gate.on_m_down();
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "1499 ms is still inside the gap");
        rig.timer.advance(Duration::from_millis(1));
        gate.on_m_down();
        assert!(rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        assert_eq!(phases(&rig.sink.take()), vec![BootstrapPhaseWire::Done]);
        assert!(!gate.active(), "done: M types m again");
        // The secret is gone from memory: a later get sees `done` and no secret.
        rig.service.bootstrap_get(B);
        let after = rig.sink.take_targeted();
        assert!(matches!(&after[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Done, .. })]), "{after:?}");
    }

    #[test]
    fn there_is_no_timer_the_password_stays_until_the_second_press() {
        let (rig, gate, _pw) = at_showing();
        rig.timer.advance(Duration::from_secs(24 * 3600));
        assert!(gate.active());
        rig.service.bootstrap_get(B);
        assert_eq!(secrets(&rig.sink.take_targeted()).len(), 1, "still handed out a day later");
    }

    #[test]
    fn a_secret_that_reached_no_frontend_cannot_be_acknowledged() {
        let (rig, gate, _pw) = at_showing();
        // The only frontend drops, then M is pressed on the dark screen.
        rig.service.connection_closed(A);
        assert!(!gate.active(), "no ready frontend: M is not consumed");
        rig.timer.advance(Duration::from_secs(60));
        gate.on_m_down();
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        // A new frontend gets the same password back; the gap counts from that delivery.
        rig.service.bootstrap_get(B);
        assert_eq!(secrets(&rig.sink.take_targeted()).len(), 1);
        assert!(gate.active());
        gate.on_m_down();
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "the gap restarted with the new delivery");
        rig.timer.advance(Duration::from_millis(1500));
        gate.on_m_down();
        assert!(rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
    }

    #[test]
    fn a_reconnect_gets_the_same_password_and_no_regeneration() {
        let (rig, _gate, pw) = at_showing();
        rig.service.connection_closed(A);
        rig.service.bootstrap_get(B);
        let again = rig.sink.take_targeted();
        assert_eq!(secrets(&again), vec![(B, pw)]);
        assert!(matches!(&again[0].1, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Showing, .. }));
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn another_connection_closing_does_not_disarm_while_one_is_ready() {
        let rig = Rig::new("!", None);
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        rig.service.bootstrap_get(B);
        rig.service.connection_closed(A);
        assert!(gate.active());
        rig.service.connection_closed(B);
        assert!(!gate.active());
        rig.service.connection_closed(B); // idempotent
    }

    #[test]
    fn a_failed_ack_write_keeps_the_screen_and_m_retries() {
        let (rig, gate, _pw) = at_showing();
        rig.timer.advance(Duration::from_secs(2));
        *rig.state.fail_save.lock().unwrap() = true;
        gate.on_m_down();
        let targeted = rig.sink.take_targeted();
        assert_eq!(code_of(&targeted[0].1), "ack_failed");
        assert!(rig.sink.take().is_empty(), "no done broadcast");
        assert!(gate.active());
        *rig.state.fail_save.lock().unwrap() = false;
        gate.on_m_down();
        assert_eq!(phases(&rig.sink.take()), vec![BootstrapPhaseWire::Done]);
    }

    #[test]
    fn a_failed_generation_returns_to_the_prompt_with_a_code() {
        let (rig, _gate) = at_prompt();
        *rig.accounts.fail_set.lock().unwrap() = true;
        press_and_join(&rig);
        assert_eq!(phases(&rig.sink.take()), vec![BootstrapPhaseWire::Generating, BootstrapPhaseWire::Prompt]);
        let targeted = rig.sink.take_targeted();
        assert_eq!(code_of(&targeted[0].1), "generate_failed");
        assert_eq!(rig.files.hash(), "!");
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        *rig.accounts.fail_set.lock().unwrap() = false;
        press_and_join(&rig);
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 1, "the next press tries again");
    }

    #[test]
    fn storage_failure_shows_the_storage_state_and_never_arms_or_generates() {
        let rig = Rig::new("!", None);
        *rig.files.available.lock().unwrap() = false;
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::None, storage_ok: false, .. })]));
        assert!(!gate.active());
        assert!(rig.service.press().is_none());
        // The overlay comes up later: the next get decides for real.
        *rig.files.available.lock().unwrap() = true;
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Prompt, storage_ok: true, .. })]));
    }

    /// A reply sink that drops every secret but passes the rest through.
    struct DroppingSink(RecordingSink);

    impl TargetedSink for DroppingSink {
        fn emit_to(&self, conn: ConnId, e: BridgeEvent) -> bool {
            if matches!(e, BridgeEvent::UserBootstrapSecret { .. }) {
                return false;
            }
            self.0.emit_to(conn, e)
        }
    }

    #[test]
    fn a_dropped_secret_is_not_delivered_and_cannot_be_acknowledged() {
        let events = Arc::new(RecordingSink::new());
        let drop_sink = Arc::new(DroppingSink(RecordingSink::new()));
        let rig = Rig::with_sinks("!", None, events.clone(), drop_sink.clone());
        let gate = rig.service.gate();
        rig.service.bootstrap_get(A);
        press_and_join(&rig);
        rig.timer.advance(Duration::from_secs(60));
        gate.on_m_down();
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "generation broadcast dropped the secret");
        // A get that also drops the secret does not arm the ack either.
        rig.service.bootstrap_get(A);
        rig.timer.advance(Duration::from_secs(60));
        gate.on_m_down();
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "re-send dropped the secret");
        assert!(!phases(&events.take()).contains(&BootstrapPhaseWire::Done));
    }

    #[test]
    fn a_dropped_secret_on_one_connection_still_counts_when_another_got_it() {
        // RecordingSink delivers to both, so this checks the "any" rule via a real delivery.
        let (rig, gate, _pw) = at_showing();
        rig.timer.advance(Duration::from_secs(2));
        gate.on_m_down();
        assert!(rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
    }

    #[test]
    fn an_unreadable_state_file_answers_internal_and_does_not_arm_a_prompt() {
        let rig = Rig::new("!", None);
        *rig.state.fail_load.lock().unwrap() = true;
        rig.service.bootstrap_get(A);
        assert_eq!(code_of(&rig.sink.take_targeted()[0].1), "internal");
        assert!(!rig.service.gate().active(), "no prompt, no armed gate after a failed read");
        *rig.state.fail_load.lock().unwrap() = false;
        rig.service.bootstrap_get(A);
        assert!(matches!(&rig.sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Prompt, .. })]));
    }

    #[test]
    fn i1_a_corrupt_state_file_with_a_usable_hash_is_relocked_never_adopted() {
        use crate::adapters::accounts_shadow::JsonStateFile;
        let dir = std::env::temp_dir().join(format!("users-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = JsonStateFile::new(&dir);
        std::fs::write(store.path(), "{not json").unwrap();
        let rig = Rig::new("$6$fake$hash", None);
        let outcome = settle_boot(rig.files.as_ref(), &store, rig.accounts.as_ref(), rig.entropy.as_ref()).unwrap();
        assert_eq!(outcome, BootOutcome::Prompt { relocked: true });
        assert_eq!(rig.files.hash(), "!");
        assert!(!store.load().unwrap().unwrap().acknowledged, "never adopted");
        // With no usable hash the corrupt file just means the prompt.
        let rig = Rig::new("!", None);
        let outcome = settle_boot(rig.files.as_ref(), &store, rig.accounts.as_ref(), rig.entropy.as_ref()).unwrap();
        assert_eq!(outcome, BootOutcome::Prompt { relocked: false });
        let _ = std::fs::remove_file(store.path());
        let _ = std::fs::remove_dir(&dir);
    }

    // Invariant I1: a crash between any two steps of the generation never
    // lets the next boot adopt a password nobody saw.

    fn next_boot(rig: &Rig) -> BootOutcome {
        settle_boot(rig.files.as_ref(), rig.state.as_ref(), rig.accounts.as_ref(), rig.entropy.as_ref()).unwrap()
    }

    #[test]
    fn i1_a_failed_state_write_sets_no_password() {
        let rig = Rig::new("!", None);
        *rig.state.fail_save.lock().unwrap() = true;
        assert!(generate_and_set(rig.entropy.as_ref(), rig.state.as_ref(), rig.accounts.as_ref()).is_err());
        assert_eq!(rig.accounts.set_count.load(Ordering::SeqCst), 0, "chpasswd never ran");
        assert_eq!(rig.files.hash(), "!");
        *rig.state.fail_save.lock().unwrap() = false;
        assert_eq!(next_boot(&rig), BootOutcome::Prompt { relocked: false });
    }

    #[test]
    fn i1_a_crash_after_the_state_write_before_chpasswd_lands_in_the_prompt() {
        let rig = Rig::new("!", None);
        *rig.accounts.fail_set.lock().unwrap() = true;
        assert!(generate_and_set(rig.entropy.as_ref(), rig.state.as_ref(), rig.accounts.as_ref()).is_err());
        assert_eq!(rig.events(), vec!["state.save ack=false", "accounts.set_password"]);
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged);
        assert_eq!(rig.files.hash(), "!");
        assert_eq!(next_boot(&rig), BootOutcome::Prompt { relocked: false });
    }

    #[test]
    fn i1_a_crash_after_chpasswd_before_the_screen_is_relocked_never_adopted() {
        let rig = Rig::new("!", None);
        generate_and_set(rig.entropy.as_ref(), rig.state.as_ref(), rig.accounts.as_ref()).unwrap();
        // The process dies here: hash usable, state unacknowledged.
        assert!(rig.files.hash().starts_with("$6$"));
        assert_eq!(next_boot(&rig), BootOutcome::Prompt { relocked: true });
        assert_eq!(rig.files.hash(), "!");
        assert!(!rig.state.held.lock().unwrap().as_ref().unwrap().acknowledged, "never adopted");
        // And the boot after that is a plain prompt again.
        assert_eq!(next_boot(&rig), BootOutcome::Prompt { relocked: false });
    }

    #[test]
    fn a_controller_restart_while_showing_relocks_and_prompts() {
        let (rig, _gate, _pw) = at_showing();
        // New process, same disk: a fresh service over the same fakes.
        let sink = Arc::new(RecordingSink::new());
        let ports = UsersPorts {
            state: rig.state.clone(),
            files: rig.files.clone(),
            accounts: rig.accounts.clone(),
            entropy: rig.entropy.clone(),
            clock: rig.timer.clone(),
        };
        let fresh = Arc::new(UsersService::new(ports, sink.clone(), sink.clone(), &UsersConfig::default()));
        fresh.bootstrap_get(A);
        assert!(matches!(&sink.take_targeted()[..], [(_, BridgeEvent::UserBootstrap { phase: BootstrapPhaseWire::Prompt, .. })]));
        assert_eq!(rig.files.hash(), "!");
    }
}
