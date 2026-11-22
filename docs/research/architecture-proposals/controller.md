# controller/ architecture proposals

Status: proposal, nothing implemented. Scope: the `controller/` crate (binary `controller-agent`). Evidence was gathered by reading every source file and running `cargo test` on a scratch copy outside the repo (57 passed, as documented). All line references are to the tree as of 2026-11-22.

## 0. Verdict up front

Recommendation: **Pattern A, a modular monolith with ports and adapters, inside the single existing crate (add a `lib.rs`, shrink `main.rs` to a composition root)**, executed in small steps with a golden JSON wire test as the first step. Stay synchronous and thread based. Do not adopt the actor model (Pattern B) and do not split into a Cargo workspace (Pattern D) yet. Take exactly one idea from the event-bus pattern (Pattern C): persistence and any future integration as event subscribers, but only when the second subscriber actually exists.

One premise in the brief needs correcting: "workspace to enable host-side testing" solves a problem that does not exist: all 57 tests already run on a plain x86 host today, including the zbus data-model tests and the fake-wpa_supplicant socket tests. What cannot be tested is the glue, not the compile target.

## 1. Current state, measured

### 1.1 Size and layout

| File | Lines | Tests | Role |
|---|---|---|---|
| `src/main.rs` | 1009 (934 before tests) | 6 | wire types, WS server, command dispatch, evdev loops, uinput, long-press, startup wiring, persistence calls |
| `src/wifi.rs` | 1467 (1055 before tests) | 26 | wpa_supplicant ctrl client, pure parsers, validation, connect state machine, service object, watcher, fake-supplicant tests |
| `src/bluetooth.rs` | 772 (564 before tests) | 18 | zbus Agent1 export, BlueZ calls, scan poll loop, pairing, classifiers |
| `src/timezone.rs` | 99 | 3 | `timedatectl` wrapper plus validation |
| `src/settings.rs` | 88 | 2 | `/var/lib/thing-revived/settings.json` load and save |
| `src/backlight.rs` | 71 | 2 | sysfs brightness |
| total | 3533 | 57 | |

There is no `tests/` directory and no `lib.rs`: everything is a private module of the binary, so nothing is reachable from integration tests. Dependencies: five direct (`evdev 0.12`, `tungstenite 0.24`, `serde`, `serde_json`, `zbus 4`), 218 lines of `cargo tree` output, 135 packages in `Cargo.lock`. The existing aarch64 release artifact is 2.76 MB (`target/aarch64-unknown-linux-gnu/release/controller-agent`); zbus and its async-io stack dominate it. `cargo-bloat` is not installed, so I did not attribute size per crate. `controller/.cargo/config.toml` only sets the two cross linkers; `+crt-static` is injected by `build/build-image.sh` via RUSTFLAGS, so a bare `cargo build --target aarch64...` is not what ships.

### 1.2 Public surface

There is no real public API because it is a bin crate, but the module interfaces are what matter. `wifi.rs` exposes about 40 `pub` items, most only because the sibling test module and `main.rs` need two or three of them (the parsers, `validate_*`, `hex_encode`, `ConnectTracker`, `Ctrl` are all consumed only inside `wifi.rs`). `bluetooth.rs` exposes free functions that take `&BtHandle` (`get_powered`, `set_powered`, `start_scan`, `stop_scan`, `pair`, `connect_device`, `list_paired_devices`, ...) plus `AgentEvent`. `backlight`, `timezone`, `settings` are free functions with hard-coded global paths (`BRIGHTNESS_PATH`, `SETTINGS_PATH`), so no seam exists to substitute a fake filesystem.

### 1.3 Threads, channels, shared state

Threads at steady state: main (TCP accept loop, `main.rs:924`), `run_buttons` (`:834`), `run_knob` (`:837`), `relay_bt_events` (`:891`), a Wi-Fi event relay (`:901`), the Wi-Fi watcher (`:909`), two threads per WS client (writer `:791`, reader loop in the handler thread `:808`), plus zbus's own executor thread. Transient: one timer thread per KEY_M press (`:676`), one thread per `bt_pair`, `bt_connect`, `bt_disconnect`, `wifi_scan`, `wifi_connect`, `wifi_connect_saved` (`:461`, `:480`, `:491`, `:543`, `:547`, `:551`), and the BT scan poll loop (`bluetooth.rs:364`).

Channels, all `std::sync::mpsc` and unbounded: one `Sender<String>` per client collected in `Arc<Mutex<Vec<Sender<String>>>>` (`main.rs:334`), `bt_events` (`:869`), `wifi_tx` (`:896`), a one-shot `Sender<bool>` for the pairing confirmation (`bluetooth.rs:89`).

Shared state: the client list; `AppState` (`main.rs:354`) holding `Option<Arc<BtHandle>>`, `Arc<Mutex<Settings>>`, `Arc<Vec<String>>` of timezones, `Arc<Wifi>`; inside `BtHandle` three more `Arc<Mutex<..>>` (pending confirm, auto-confirm flag, seen set); inside `Wifi` three `Mutex` fields and two `AtomicBool` busy flags (`wifi.rs:664`); the KEY_M `Arc<Mutex<Option<Instant>>>` generation marker (`main.rs:634`).

### 1.4 Where the protocol lives

Everything about the wire is in `main.rs`: `BridgeEvent` (23 variants, `:79-189`), the wire mirror structs (`:191-278`), `From<wifi::WifiEvent>` (`:206`), `ClientCommand` (22 variants, `:283-332`), `Secret` (`:250`). Parsing is `serde_json::from_str::<ClientCommand>` at `:811`, a malformed message only produces an `eprintln!` (`:813`), never an error reply. Dispatch is one 135-line `match` in `handle_client_command` (`:422-557`). Replies are not correlated to requests: every event is broadcast to all clients (`broadcast`, `:336`), and there are no request ids. The three error events `bt_error`, `settings_error`, `wifi_error` are structurally identical `{context, message}`.

### 1.5 Persistence

Three independent mechanisms. `settings.json` (brightness, timezone) is written inline from the handler while holding the settings lock (`main.rs:513-516`, `:526-529`), through `settings.rs:54` using plain `fs::write`, not write-to-temp-then-rename, so a power cut mid-write can leave a truncated file (load tolerates it by falling back to defaults, `settings.rs:40`, which silently loses the saved timezone). Startup re-applies brightness and timezone (`main.rs:850-867`). Wi-Fi networks are persisted by wpa_supplicant `SAVE_CONFIG` (`wifi.md`). Bonded Bluetooth devices are BlueZ's job. That split is correct; the weakness is only that the first one is invoked from the handlers.

### 1.6 Error handling

`Result<_, String>` everywhere (backlight 5, bluetooth 10, timezone 3, wifi 11 signatures); `main.rs` has none. The caller must know which wire error variant and which `context` string to wrap it in, so the same 3-line `match` plus `"Bluetooth unavailable"` arm is copy-pasted eleven times (`main.rs:424-507`). Failure at startup is mixed: `open_device` panics after 10 tries (`:589`), uinput setup uses `expect` (`:596-606`), `bind` uses `expect` (`:921`), while BlueZ failure degrades to `bt: None`. A panic in any thread that holds a `Mutex` poisons it and the next `.lock().unwrap()` (about 70 of them) panics in turn, notably the client list lock inside `broadcast` (`:345`), which every thread calls. Logging is bare `println!`/`eprintln!` (63 call sites) with a `[bridge]` prefix, and `broadcast` prints every event including every knob tick (`:344`), which goes to journald on a flash-backed device.

### 1.7 What is unit-testable today versus hardware

Testable and tested: wire serialization shape for Wi-Fi events and command parsing (6 tests in `main.rs`), `Secret` redaction, every wpa_supplicant parser and validator, the connect tracker, the whole connect/scan/forget flow against a fake control socket (`wifi.rs:1299`, using a `WPA_CTRL_DIR` env var as the seam plus a static `ENV_LOCK`, `:1357`), BlueZ ObjectManager parsing with synthetic variants, classifiers, brightness and timezone validators, settings JSON roundtrip.

Not testable: the whole command router (needs real `AppState` with a real `Wifi`, BtHandle is a zbus `Connection`, backlight and timezone are global paths and subprocesses); the `run_buttons` logic (grab, replay filter, long-press) because it is welded to `evdev::Device` and `VirtualDevice`; the client plumbing in `handle_client`; BlueZ calls other than the pure extractors; `timedatectl` and `systemctl` calls. Note the three KEY_M timer tests (`bluetooth.rs:656`, `:671`, `:763`, sitting in the wrong file) exercise a local `Mutex<Option<Instant>>` and assert on it; they never call production code, so they test nothing. The long-press logic has zero real coverage even though it is the "hard recovery" feature.

## 2. Assessment

### 2.1 Smells with evidence

1. **`main.rs` is five modules in one file.** Wire schema, WS transport, router, input device loops, and boot sequencing all live in 1009 lines. The doc comment at `bluetooth.rs` says it "deliberately has no WS/serde dependency", and `main.rs:260-266` explains the mirror struct pattern, so the intended layering exists in prose but not in the module tree.
2. **The router has no seam.** `handle_client_command` takes `&AppState` made of concrete types, so it cannot be tested, and each new backend adds a field to `AppState` plus another arm family.
3. **Per-arm boilerplate and ad hoc concurrency policy.** Which commands run inline and which spawn a thread is decided by hand per arm with a prose comment (`:456`, `:475`, `:499`, `:539`). The policy is real and correct, but it is encoded as scattered `thread::spawn` calls instead of one declared table.
4. **Pure logic is trapped inside effectful code.** The KEY_M long-press state machine is inline in the evdev loop. This is the highest value-per-line extraction target because it is a safety feature with no test.
5. **Stringly typed errors and three duplicate wire error events.**
6. **Unbounded fan-out under a global lock.** `broadcast` holds the client-list mutex while sending to every client channel (`:345-346`); channels are unbounded (`:787`), so a stalled WebSocket peer (the writer thread blocks in `ws_write.send`) accumulates every knob tick and Wi-Fi status in memory until the 10 s ping fails. Low risk with one Chromium client on 512 MB, but unbounded on a `0.0.0.0` bind (`:921`), which `wifi.md` already flags.
7. **Global state as test seam**: env var `WPA_CTRL_DIR` and `static ENV_LOCK`; hard-coded sysfs and settings paths.
8. **Non-atomic settings write** (above).
9. **Misplaced tests**: timer tests in `bluetooth.rs`.
10. **Duplicated truth for the protocol**: `docs/wifi.md` carries a wire table, `main.rs` carries doc comments that say they are "the source of truth, not any example quoted in the design doc" (`:77-78`), `frontend/app.js` (1573 lines) hand-parses. Drift has already occurred once, which is why the comment exists.

### 2.2 Real pain points, ranked

1. Adding the next settings backend costs edits in four places (`BridgeEvent`, `ClientCommand`, `AppState`, the router match) plus startup wiring in `main`, all in the same 1000-line file. This will recur: the backlog has font size and theme (frontend only, but they will want persisted settings), settings backup, idle clock overlay, BT PAN (new BlueZ network calls), health endpoint, and any future integration.
2. New input mappings (the "button to action" idea from the superseded design) have no home: the evdev loop hard-codes "replay M and Enter, broadcast everything, M long press restarts terminal-agent".
3. The command router and the input state machine are untested.
4. Protocol drift across three consumers.

Not pain points: thread count (about 12 steady threads is trivial), binary size, the sync model, or compile ability on host. I am explicitly not counting "sync versus async" as a pain point: the workload is a handful of blocking I/O sources at human interaction rates, and the blocking sections (Wi-Fi 40+25 s connect, BT `Pair()` that blocks waiting for the user) are naturally expressed as blocking code on a dedicated thread.

### 2.3 Sync versus async, decided

Keep it sync. An async runtime would add a runtime dependency and a rewrite of the parts that work (wifi event waits with timeouts, `UnixDatagram` with read timeouts, zbus blocking API wrapping async-io which is already linked). It buys nothing measurable: no thousands of connections, no latency target tighter than a knob tick. The one place async would genuinely simplify is the zbus signal subscription (the scan loop polls every 2 s, `bluetooth.rs:521`), and zbus blocking already supports `receive_*` iterators without introducing a runtime into our code. Revisit only if a future network client needs TLS plus a persistent subscription, and even then a dedicated blocking thread with a sync client is the first option to try.

## 3. Pattern A: modular monolith with ports and adapters (hexagonal), one crate

### 3.1 Idea

Separate three things that are currently interleaved: the wire protocol (what clients see), the application logic (what each command means), and device adapters (evdev, sysfs, BlueZ, wpa_supplicant, systemctl, filesystem). Application logic depends on small traits ("ports") owned by the application; adapters implement them; `main.rs` is the only place that knows the concrete types. One crate with `lib.rs` plus a thin `main.rs`, so integration tests under `tests/` can reach everything.

### 3.2 Target tree

```
controller/
  Cargo.toml                    add [lib] name = "controller_agent"; no new deps
  src/
    main.rs                     composition root only: build adapters, wire hub + router + threads (~120 lines)
    lib.rs                      pub mod declarations; the test entry point
    protocol/
      mod.rs                    re-exports
      command.rs                ClientCommand, Secret (moved from main.rs 250-332)
      event.rs                  BridgeEvent plus wire structs (main.rs 79-278), serde only
      golden.rs                 #[cfg(test)] golden JSON for every command and event variant
    app/
      mod.rs
      router.rs                 Router::handle(Command) -> dispatch to services; the single match
      policy.rs                 Command::execution() -> Inline | Blocking, replaces scattered thread::spawn
      hub.rs                    EventHub: client registry, broadcast, bounded per-client queue
      error.rs                  AppError {context, kind, message}, kind = Unavailable | Invalid | Failed
      startup.rs                re-apply persisted brightness and timezone (main.rs 850-867)
    ports.rs                    traits: Backlight, TimezoneCtl, SettingsStore, BluetoothPort, WifiPort,
                                Restarter, EventSink, Clock
    services/
      display.rs               brightness get/set + persistence (from main.rs 509-521, backlight validate)
      timezone.rs              get/set/list + persistence (from main.rs 522-537, timezone validate)
      bluetooth.rs             command handling for BT, paired-list refresh rule (main.rs 363-416, 424-508)
      wifi.rs                  command handling for Wi-Fi, WifiEvent -> BridgeEvent (main.rs 206-245)
      input.rs                 ButtonPolicy: which codes replay, broadcast; LongPress state machine
    adapters/
      evdev_input.rs           open_device, grab, fetch_events loops for event0, event1 (main.rs 576-757)
      uinput.rs                VirtualDevice builder + emit (main.rs 592-607)
      ws_server.rs             accept loop, handle_client, writer thread, ping (main.rs 770-825, 924-932)
      sysfs_backlight.rs       impl Backlight (from backlight.rs, path injectable)
      timedatectl.rs           impl TimezoneCtl (from timezone.rs, command runner injectable)
      json_settings.rs         impl SettingsStore, atomic write-rename (from settings.rs)
      bluez.rs                 impl BluetoothPort: zbus Agent1, scan loop (bluetooth.rs I/O half)
      wpa_ctrl.rs              Ctrl socket client (wifi.rs 556-627)
      systemctl.rs             impl Restarter (main.rs 686-697)
    domain/                     pure, no I/O, no serde, no zbus types in signatures
      keys.rs                  KEY_* consts and replay table (main.rs 57-62)
      longpress.rs             pure state machine: on_press(now) -> Armed(deadline), on_release, on_tick
      brightness.rs            validate (backlight.rs 36)
      timezone.rs              validate_timezone
      bt_classify.rs           class/icon/uuid classifiers, address<->path (bluetooth.rs 210-281)
      bt_paired.rs             extract_paired_devices (bluetooth.rs 402-435)
      wifi/
        scan.rs                parse_scan_results, dedup, security_from_flags, dbm_to_pct
        status.rs              parse_status, derive_state, parse_rssi, StatusInfo
        validate.rs            validate_ssid, validate_password, hex_encode, network_settings
        events.rs              WpaEvent, parse_event
        tracker.rs             ConnectTracker, FailKind
  tests/
    router_flow.rs              fake ports + hub: command in, events out
    wire_golden.rs              every variant serializes to the committed golden JSON
    input_replay.rs             fake evdev event stream + fake uinput sink + fake clock
```

Everything except the pure `domain/` and `protocol/` modules moves rather than changes; the `domain/wifi/*` and `domain/bt_*` code is a mechanical split of `wifi.rs` and `bluetooth.rs` with their existing tests traveling along.

### 3.3 Current to target mapping

| Current | Target |
|---|---|
| `main.rs:79-332` | `protocol/event.rs`, `protocol/command.rs` |
| `main.rs:334-347` broadcast, clients | `app/hub.rs` |
| `main.rs:354-361` AppState | replaced by `Router` fields (services holding `Arc<dyn Port>`) |
| `main.rs:363-557` | `services/bluetooth.rs`, `services/display.rs`, `services/timezone.rs`, `services/wifi.rs`, `app/router.rs` |
| `main.rs:576-757` | `adapters/evdev_input.rs`, `adapters/uinput.rs`, `services/input.rs`, `domain/longpress.rs` |
| `main.rs:759-825` | `adapters/ws_server.rs` |
| `main.rs:827-933` | `main.rs` (composition) plus `app/startup.rs` |
| `settings.rs`, `backlight.rs`, `timezone.rs` | `adapters/json_settings.rs`, `sysfs_backlight.rs`, `timedatectl.rs` plus `domain/*` validators |
| `bluetooth.rs` | `adapters/bluez.rs` plus `domain/bt_*.rs` |
| `wifi.rs` | `adapters/wpa_ctrl.rs`, `services/wifi.rs` (Wifi service object), `domain/wifi/*` |

### 3.4 Key traits and types

```rust
// ports.rs
pub trait Backlight: Send + Sync { fn max(&self) -> Result<u32, AppError>; fn get(&self) -> Result<u32, AppError>; fn set(&self, v: u32) -> Result<(), AppError>; }
pub trait SettingsStore: Send + Sync { fn load(&self) -> Settings; fn save(&self, s: &Settings) -> Result<(), AppError>; }
pub trait TimezoneCtl: Send + Sync { fn list(&self) -> Vec<String>; fn get(&self) -> Result<String, AppError>; fn set(&self, tz: &str) -> Result<(), AppError>; }
pub trait Restarter: Send + Sync { fn restart(&self, unit: &str) -> Result<(), AppError>; }
pub trait EventSink: Send + Sync { fn emit(&self, e: BridgeEvent); }   // implemented by EventHub and by a Vec recorder in tests
pub trait BluetoothPort: Send + Sync { /* get_powered, set_powered, start_scan, stop_scan, pair, submit_confirm, list_paired, connect, disconnect, forget */ }
// app/error.rs
pub enum ErrorKind { Unavailable, Invalid, Failed }
pub struct AppError { pub kind: ErrorKind, pub message: String }
```

`Wifi` already is a service with an internal `Sender<WifiEvent>`, so `WifiPort` can start as that existing struct behind a thin trait, and the fake-supplicant tests stay as adapter-level integration tests. Where BlueZ is absent, `main` injects an `UnavailableBluetooth` adapter that returns `AppError::Unavailable` for every call, which deletes the `Option<Arc<BtHandle>>` and the eleven copy-pasted `None =>` arms: the router then maps `AppError` to the correct wire error variant in one place by command family.

### 3.5 Message flow

Input: evdev thread reads an event, `services/input.rs` applies the policy (broadcast always; replay if code in {M, Enter}; feed `LongPress`), emits `BridgeEvent::Button` to the sink, writes to the `Uinput` port, asks `Restarter` when `LongPress` fires. Commands: `ws_server` reads text, `protocol::command` parses, `Router::handle(cmd)` consults `policy::execution(&cmd)`; `Inline` runs on the reader thread, `Blocking` is handed to a small spawn helper (still `std::thread::spawn`, one place); the service calls ports and emits events (`EventSink`), the hub fans out to client queues. Persistence: services call `SettingsStore` after a successful device call, exactly as today but behind a port and with an atomic write.

### 3.6 Concurrency model

Unchanged: threads and mpsc. Two small improvements ride along without changing the model: a bounded per-client queue (drop-oldest for knob and status, keep the rest) in `EventHub`, and `parking_lot`-style non-poisoning avoided by simply using `lock().unwrap_or_else(|e| e.into_inner())` through a tiny helper in one place. No new crates.

### 3.7 Testing strategy, what becomes testable without hardware

Router tests with fake ports: every command produces the expected events, `Unavailable` mapping, persistence called exactly after a successful set and not after a failed one, paired-list refresh after connect and after pair success. Input policy tests with a scripted event iterator, a fake uinput recording emits, and a fake clock: M and Enter replay, 1 to 4 and Esc do not, long press fires once at 1500 ms, release before the deadline cancels, new press supersedes (this finally tests the real logic instead of a local mutex). Settings store tested on a tempdir including atomic write and corrupt-file fallback. Golden wire tests for all 45 messages. Still hardware only: the evdev grab itself, uinput creation, BlueZ calls, real wpa_supplicant behaviour.

### 3.8 Dependencies and binary size

None added. `dyn` dispatch on a handful of ports is negligible. Expect the binary within a few KB of the current 2.76 MB; with `lto = true` and `opt-level = "z"` generics versus `dyn` makes no practical difference at this size.

### 3.9 Pros and cons

Pros: directly attacks pain points 1, 2, 3; zero new dependencies; each step is a move of existing code, so the 57 tests keep passing; creates the natural place for a future integration (a new adapter plus maybe a new service) and for new settings backends (a service file, a port, an adapter, two enum variants); the `protocol/` module becomes a clean unit to later back with a spec.

Cons: about 35 files for 3.5k lines is more ceremony than the code strictly needs, and the `domain/` versus `services/` versus `adapters/` split will be argued file by file (wifi.rs has the `Wifi` service object that mixes all three). Traits invite over-abstraction: resist adding a port that has exactly one adapter and no test using a fake. Each port should justify itself by a test it enables (Backlight, SettingsStore, TimezoneCtl, Restarter, EventSink, input source and sink are justified; do not add a port for the wpa ctrl socket, its fake socket test already works).

### 3.10 Migration steps (build and 57 tests green after every step)

1. Add `tests/` golden wire test by serializing every `BridgeEvent` and parsing every `ClientCommand` from a committed fixture file, before moving anything. This is the behaviour lock for all later steps. (S)
2. Add `lib.rs`; `main.rs` becomes `fn main() { controller_agent::run() }` initially holding the old code, to prove the lib/bin split. Existing in-file tests keep working. (S)
3. Extract `protocol/` (move `BridgeEvent`, `ClientCommand`, `Secret`, wire structs, the six main tests). Pure move. (S)
4. Extract `domain/longpress.rs` with an injected-time API and replace the inline logic in `run_buttons`; delete the three fake timer tests from `bluetooth.rs` and replace them with real tests. This is the single most valuable change. (S)
5. Introduce `AppError` and convert the `Result<_, String>` signatures module by module (backlight, timezone first, then bluetooth, then wifi); `From<AppError>` for the existing strings keeps call sites compiling during the transition. (M)
6. Introduce `EventHub` (move `broadcast` and client registry) and `EventSink`; services receive a sink instead of `Clients`. (S)
7. Define `Backlight`, `TimezoneCtl`, `SettingsStore` ports with sysfs, timedatectl, json adapters, and `services/display.rs`, `services/timezone.rs`; make the settings write atomic. Router tests for these arms. (M)
8. Introduce the `BluetoothPort` trait with the real `bluez` adapter and the `UnavailableBluetooth` adapter; collapse the eleven `None =>` arms. (M)
9. Move `evdev` and `uinput` loops into adapters behind a small `InputSource` plus `KeyReplay` trait pair; add the scripted-input tests. (M)
10. Split `wifi.rs` into `domain/wifi/*` and `adapters/wpa_ctrl.rs` plus `services/wifi.rs`. Optional and last: it is the largest, best-tested, least-broken file. (M)

Effort: M overall (about 4 to 6 focused sessions), steps 1 to 4 deliver most of the value in about one. Risk: low, because step 1 pins the wire and every other step is a move or an additive seam; the one behavioural change is the atomic settings write and the bounded client queue, both separately reviewable.

## 4. Pattern B: actor per device with a central router

### 4.1 Idea

One owner thread per physical or logical device, each with a private inbox (`mpsc::Receiver<Msg>`) and exclusive ownership of its resource and state: no `Mutex` in application code. A router thread owns the client list and settings and is the only thing that talks to WebSocket writers. Commands are routed to the owning actor by family; actors send `Event` messages back to the router.

### 4.2 Target tree

```
controller/src/
  main.rs                 spawn actors, wire inboxes
  protocol/{command,event}.rs      as in Pattern A
  runtime/
    mod.rs                Actor trait: fn run(self, inbox: Receiver<Self::Msg>); spawn helper; shutdown signal
    router.rs             RouterActor: owns clients, settings; Msg::{ClientConnected, ClientGone, Command(ClientId, Command), Event(BridgeEvent)}
    gateway.rs            WsGateway: accept loop and per-client reader; hands writers to the router
  actors/
    input.rs              InputActor: owns event0, uinput, long-press; own timer via recv_timeout
    knob.rs               KnobActor: owns event1
    bluetooth.rs          BluetoothActor: owns BtHandle, pending confirmation
    wifi.rs               WifiActor: owns Ctrl, last_scan, iface; tick via recv_timeout replaces the watcher thread
    display.rs            DisplayActor: backlight, timezone, owns SettingsStore
  devices/                thin drivers (sysfs, evdev, zbus calls, wpa ctrl) with no threads
  domain/                 same pure modules as Pattern A
```

### 4.3 Mapping

`main.rs` wire types go to `protocol/`; each of `wifi.rs`, `bluetooth.rs`, `backlight.rs`, `timezone.rs`, `settings.rs` splits into a driver (`devices/`) and an actor (`actors/`); `run_buttons`/`run_knob` become actors; the `Mutex` and `AtomicBool` fields in `Wifi` and `BtHandle` disappear.

### 4.4 Types and flow

`enum WifiMsg { Cmd(WifiCommand), Tick, ScanDone, ConnectDone(..) }` and similar per actor; `Router` holds `HashMap<ActorId, Sender<_>>` and `Vec<ClientWriter>`. Input event: `InputActor` emits `Event(Button)` to router, which fans out. Command: gateway posts `Msg::Command` to router, router sends it to `WifiActor`, which later posts `Event` back.

### 4.5 Concurrency model

Share-nothing message passing; each actor serially processes its inbox; fan-out only at the router.

### 4.6 The decisive problem

Several operations are long and block on an event that arrives through the same actor's inbox, which deadlocks or head-of-line-blocks the actor. `Device1.Pair()` blocks until the user answers `bt_pairing_confirm` (`bluetooth.rs:130-157` waits on a channel fed by `submit_confirm`); if `BluetoothActor` runs `pair` inline, it cannot dequeue the confirm message that would unblock it. `wifi_connect` runs up to 40 + 25 s (`wifi.rs:979`); during that the actor could not serve `wifi_get`, `wifi_disconnect`, or the 3 s status tick. The fix is either helper threads per long op (re-inventing today's `thread::spawn` plus busy guards, with message plumbing added on top) or rewriting those flows as explicit state machines driven by events, which is a rewrite of `connect_flow` and the BT pair flow, the two most hardware-sensitive and least verified parts. The existing `ConnectTracker` is already half a state machine but `drive_connection` is a blocking loop.

### 4.7 Testing

Each actor can be tested by feeding messages and reading outputs, which is clean, and the router is testable with channels as fakes. But the same coverage is available in Pattern A with plain function calls and fewer moving parts.

### 4.8 Dependencies, effort, risk

No new crates. Effort L (the state-machine rewrites dominate). Risk high: the Wi-Fi flow is "built, not yet verified on hardware" (`docs/development.md`) and the BT pairing flow is the one that took live-hardware iteration; a rewrite changes timing behaviour that cannot be tested on the host.

### 4.9 Pros and cons

Pros: no shared mutable state in app code, serialization per device is implicit (no busy guards), a clean story for shutdown and supervision, scales to many devices. Cons: the deadlock above; rewrites verified-on-hardware behaviour; the system has 5 devices, and a router hop adds latency and a clone for every knob tick. The actor model pays off when state sharing is the dominant bug source. Here state sharing is modest (8 mutexes, no known deadlock, no reported race) and the one real concurrency risk, the KEY_M timer generation check, is solved by a pure state machine in Pattern A, not by actors.

### 4.10 Migration steps

(1) golden test; (2) `protocol/` extraction; (3) introduce `Actor` helper and convert `KnobActor` (trivial); (4) `DisplayActor` and `InputActor`; (5) router thread replaces `broadcast`; (6) `BluetoothActor` with helper threads for `pair` and `connect`; (7) `WifiActor`, last, with helper threads for scan and connect. Steps 1 to 5 are safe and are largely the same as Pattern A steps; 6 and 7 are where the risk sits. Build and tests stay green at each step if the old and new paths are not mixed within one device.

## 5. Pattern C: event bus plus command handlers (CQRS-lite, typed protocol)

### 5.1 Idea

Treat everything as one of two typed message kinds. Commands (client intent, `protocol::Command`) go to exactly one handler registered for that command family; handlers produce domain events (`DomainEvent`: `BrightnessChanged`, `TimezoneChanged`, `WifiStatusChanged`, `ButtonPressed`, ...). A small synchronous in-process bus delivers every event to subscribers: the WS hub (translates to wire events), the settings persister (saves on `*Changed`), a log subscriber, and later a second event subscriber or an action-mapping engine (button to action). Queries read from a `Snapshot` read model that subscribers keep current, so a newly connected client gets full state in one call instead of ten `*_get` round trips.

### 5.2 Target tree

```
controller/src/
  main.rs                 composition
  lib.rs
  protocol/{command,event,wire_map}.rs    wire types and DomainEvent -> BridgeEvent mapping
  bus.rs                  Bus: subscribe(Fn(&DomainEvent)), publish; sync, in-caller-thread
  commands/
    mod.rs                trait CommandHandler { fn handle(&self, cmd: Command, bus: &Bus) }
    bluetooth.rs          BtHandler
    display.rs            BrightnessHandler
    timezone.rs           TimezoneHandler
    wifi.rs               WifiHandler
  events.rs               DomainEvent enum (no serde)
  subscribers/
    ws_hub.rs             fan-out to clients
    persist.rs            DomainEvent::{Brightness,Timezone}Changed -> SettingsStore
    snapshot.rs           read model, answers the *_get family and new-client sync
    mapper.rs             (future) input mapping, (future) a second integration adapter
  devices/                evdev, uinput, sysfs, zbus, wpa ctrl as in Pattern A adapters
  domain/                 pure logic as in Pattern A
```

### 5.3 Mapping

Same source-to-target moves as Pattern A for devices and domain; the new pieces are `bus.rs`, `events.rs`, `subscribers/*`. `AppState` disappears; `relay_bt_events` and the Wi-Fi relay thread become the bus plus the wire map; the `save_settings` calls leave the handlers and move into `persist.rs`.

### 5.4 Flow, concurrency

`ClientCommand` goes to a handler, which calls a device and publishes events. Publication runs subscribers synchronously on the publishing thread (so a slow subscriber stalls the publisher; subscribers must be non-blocking and push to a queue if they do I/O). Threading model identical to today.

### 5.5 Testing

Handlers are tested by asserting the published events on a recording subscriber; persistence is tested by publishing `BrightnessChanged` and observing the store; snapshot by replaying events. Same level as Pattern A, with the added ability to unit test the persister in isolation.

### 5.6 Dependencies, effort, risk

None. Effort M to L. Risk medium: control flow becomes implicit (who reacts to this event?), and every knob tick allocates a `DomainEvent` plus a `BridgeEvent` plus a JSON string before today's single `serde_json::to_string`.

### 5.7 Pros and cons

Pros: the right shape if the feature list grows fan-out (persist plus a second publisher plus log plus UI for the same event), fixes "new client needs state" cleanly, makes a future integration and input mapping additive. Cons: it is two message types plus a registry for a system whose features are 90 percent request/response against one device, so most events would have exactly one subscriber (the hub). Wire protocol today is already a de facto event stream broadcast to all clients, so the hub subscriber would be a near-identity mapping. Honestly overlaps with Pattern A for everything except the bus and snapshot; the value is conditional on a second event subscriber actually landing. Also note: no request ids exist on the wire, which the pattern encourages adding, but that is a protocol breaking change to coordinate with the frontend.

### 5.8 Migration steps

Same as Pattern A steps 1 to 4, then: introduce `DomainEvent` plus `Bus` with the hub as sole subscriber (the wire map is the old `From` impls); move persistence into a subscriber; convert one backend at a time to publish domain events; add `Snapshot` last. Each step keeps the build green; no step is a rewrite.

## 6. Pattern D: Cargo workspace with separate crates

### 6.1 Idea

Hard boundaries enforced by the compiler: `controller-protocol` (serde types only), `controller-core` (domain, services, ports, no I/O crates), `controller-adapters` (evdev, zbus, sysfs, wpa), `controller-agent` (bin). Optionally `controller-protocol` is also shared by `daemon/` and a test-only client.

### 6.2 Target tree

```
controller/
  Cargo.toml                    [workspace], resolver 2, members
  Cargo.lock                    moves here (unchanged path if the workspace root is controller/)
  .cargo/config.toml
  crates/
    protocol/    Cargo.toml (serde, serde_json)   src/{lib,command,event,secret}.rs
    core/        Cargo.toml (protocol)            src/{lib,ports,router,hub,error}.rs, services/*, domain/*
    adapters/    Cargo.toml (core, evdev, zbus)   src/{evdev_input,uinput,sysfs_backlight,timedatectl,json_settings,bluez,wpa_ctrl,systemctl}.rs
    agent/       Cargo.toml (bin controller-agent; core, adapters, tungstenite)  src/{main,ws_server}.rs
```

### 6.3 Mapping

Pattern A's `protocol/`, (`app/`, `services/`, `ports.rs`, `domain/`), `adapters/` become the three library crates; `ws_server` and `main` stay in the bin.

### 6.4 Concurrency, flow, testing

Identical to Pattern A. Added benefits: `core` cannot accidentally import `zbus` or `evdev` (compile-time layering), faster incremental test builds of `core`, and `protocol` could be depended on by another crate. Because `core` has no zbus, `cargo test -p controller-core` skips building the largest dependency, which matters on a small CI runner and in a Docker cross build.

### 6.5 Costs specific to this repo

CI points at `controller/Cargo.lock` and `controller/target` (`.github/workflows/ci.yml:50-51`), `.gitignore` ignores `/controller/target/`, `build/build-image.sh` hard-codes the controller path and the per-target artifact location and injects `+crt-static` through RUSTFLAGS; a workspace rooted at `controller/` keeps all three paths valid but every crate must stay consistent with the `+crt-static` build and the `[profile.release]` settings (profiles must live in the root manifest). The repo has no root workspace, `daemon/` has its own `Cargo.lock` and its own protocol handled as untyped `serde_json::Value` (`daemon/src/main.rs:440`), so the sharing argument is weak today: the two daemons share no types, only a convention (`type` tag, permissive ignore).

### 6.6 Pros and cons

Pros: strongest boundaries, protocol reusable, partial test builds. Cons: four manifests for 3.5k lines, the dependency list (five direct crates) is too small to benefit from per-crate feature isolation, and the visibility rewrites (`pub(crate)` becomes `pub` at every crate seam) add churn. The "host-side testing" goal is already met. The boundary enforcement can be had cheaply in Pattern A by a module convention plus a unit test that greps `domain/` and `app/` for `use zbus`/`use evdev` (about 10 lines).

### 6.7 Migration, effort, risk

Do Pattern A steps 1 to 9 first (the crate split is then a mechanical `git mv` of finished directories plus manifest work), then: create the workspace root with `protocol` only, keep everything else in `agent`; move `core`; move `adapters`. Effort L when counted after A (M for the split itself, plus CI and build-script verification, which cannot be tested without running the Yocto flow). Risk medium, entirely in the build pipeline rather than the code.

## 7. Comparison

Scores are 1 (worst) to 5 (best) for this codebase specifically.

| Criterion | A: hexagonal single crate | B: actors | C: bus + handlers | D: workspace |
|---|---|---|---|---|
| Fixes untestable router and input logic | 5 | 4 | 5 | 5 |
| Cost of adding the next settings backend | 4 | 3 | 4 | 4 |
| Fit for a future integration / input mapping later | 3 | 3 | 5 | 3 |
| Preserves verified hardware behaviour | 5 | 1 | 4 | 5 |
| New dependencies | none | none | none | none |
| Binary size impact | none | none | slight (allocs only) | none |
| Build and CI pipeline impact | none | none | none | real (paths, RUSTFLAGS, cache) |
| Ceremony versus 3.5k lines | 3 | 3 | 2 | 1 |
| Effort | M | L | M to L | L (after A) |
| Risk | low | high | medium | medium |

Weighted result: A wins on risk and on payoff per line; C wins only on the future-integration axis, which is a speculative feature; B loses on a concrete deadlock-class problem; D is a later, optional consequence of A.

## 8. Recommendation

**Do Pattern A, in the 10 steps above, and stop wherever the marginal step stops paying.** Steps 1 to 4 (golden wire test, lib/bin split, protocol extraction, the long-press state machine) are the high-yield part, roughly a day, and fix the only genuinely untested safety feature. Steps 5 to 8 (typed errors, hub, ports, Bluetooth unavailable adapter) fix the boilerplate and make the router testable. Steps 9 and 10 are optional.

Reasoning chain, with the alternative I considered and the property that ruled it out:
- Bus-first (C): ruled out for now because the feature that would justify it (a second event subscriber) is not on the committed backlog. A is a strict prerequisite for C anyway, since C's handlers need the same ports. Adding the bus later is cheap if `EventSink` already exists, which step 6 gives for free.
- Actors (B): ruled out by the blocking-reply property of `Pair()` and `wifi_connect`, as shown in 4.6.
- Workspace (D): ruled out by "five dependencies, one consumer of the protocol types, build pipeline with hard-coded paths". Revisit when a second crate needs `protocol` (for instance a shared type with `daemon/`) or `core` test compile time bothers you.

**Pattern to avoid: B (actors).** Highest risk, rewrites the two flows that are least verifiable off-device, and replaces a working busy-guard design with message plumbing that still needs helper threads.

**What not to refactor:**
- The wpa_supplicant parsers, validators, tracker and their 26 tests (only move them).
- The fake-supplicant end to end tests; do not replace them with a trait fake, they test the real text protocol handling.
- `Secret` and its redaction test; keep the exact `Debug` behaviour.
- The thread-per-client WS model, `try_clone` split reader and writer, and the sync tungstenite stack.
- The zbus `Agent1` export and `connect_and_register_agent` internals (only move).
- The EVIOCGRAB plus selective uinput replay semantics and the 1500 ms long-press constant: extract the logic, change no behaviour.
- The JSON wire shapes. Step 1's golden test must stay byte-for-byte green through the whole refactor; any protocol cleanup (merge the three error events, add request ids, rename `value` fields, the `{}` empty-struct variants) is a separate versioned change coordinated with the frontend, never a side effect of this refactor.
- Do not introduce `anyhow`, `thiserror` or `tracing`: a 25-line hand-written `AppError` and a 10-line log macro are enough, and each crate adds binary size and build time on a device with a 2.76 MB budget mindset. Reconsider only if `AppError` grows past three kinds.

Small, independent fixes worth doing regardless: atomic settings write; do not `println!` every knob tick (`main.rs:344`); bind to `127.0.0.1` (flagged in `wifi.md`, a deployment decision for the owner, not a refactor; done as of 2026-11-22, default `127.0.0.1:8765`, override `CONTROLLER_BIND`); drop the three tautological timer tests when step 4 lands.

Confidence: high that A is the right first move and B is wrong; medium that C will never be needed (depends on a second subscriber appearing, which is not in the backlog); medium-low on the effort figures, since I have not touched the code.

## 9. Shared concerns for the daemon/ and frontend/ analyses

### 9.1 What exists

Two unrelated JSON-over-WebSocket protocols: controller on `ws://0.0.0.0:8765` (22 commands, 23 events, serde-typed), terminal-agent on 8766 (about 10 messages, hand-dispatched on `serde_json::Value`, `daemon/src/main.rs:440`). Both use a `type` discriminator, snake_case, permissive ignore of unknown input, no versioning, no request ids. The frontend (`app.js`, 1573 lines, no build step, classic scripts, vendored xterm.js, node `--test` for pure logic) parses both by hand. The only written contracts are prose: `docs/wifi.md` tables, `docs/terminal.md`, and doc comments on the Rust enums. Evidence of drift: `main.rs:77-78` says its field shapes win over the design doc's examples.

### 9.2 Items to coordinate with siblings

1. One decision on spec ownership: where the spec lives (suggest `protocol/` at the repo root, one directory, one spec per daemon plus shared envelope conventions) and which side is authoritative.
2. Shared conventions to fix once: discriminator name (`type`), case, error event shape (one `error {domain, context, code, message}` instead of `bt_error`, `settings_error`, `wifi_error`, plus a stable code like Wi-Fi already has), optional request id echoed back, protocol version handshake or at least a `hello` event, unknown message handling (reply with an error vs ignore).
3. Initial-state sync: today every client sends N `*_get` commands; a `state_snapshot` event would help both daemons and the frontend reconnect logic.
4. Port binding and trust: `0.0.0.0` on both; a loopback bind plus an origin check would harden both with the same change.
5. Test fixtures: one set of golden JSON messages consumed by the Rust tests (both daemons) and the node tests (frontend), which is the cheapest anti-drift mechanism available regardless of generation.
6. Secret handling convention (the `Secret` type, the `password` field never echoed) should be written into the spec as a field-level annotation.

### 9.3 Spec-first with generated contracts, evaluated honestly

The user preference is spec-first with generated contracts such as TypeSpec. What I could confirm: TypeSpec has a first-party JSON Schema emitter (`@typespec/json-schema`) that supports discriminated unions via the `@discriminator` decorator and the `polymorphic-models-strategy: oneOf` option (added around 1.7), and a community AsyncAPI 3.1 emitter exists (`LarsArtmann/typespec-asyncapi`, third party, young). I did not find a first-party TypeSpec emitter that produces serde types for WebSocket message models (the Rust emitters I know of target HTTP client SDKs), so treat "TypeSpec to Rust types" as an unproven path needing a spike, not a given. Likely pipeline if it works: TypeSpec to JSON Schema, then JSON Schema to Rust with a generator such as `typify`, and to a `.d.ts` with `json-schema-to-typescript` for JSDoc `@ts-check` in the buildless frontend. Unverified risks in that pipeline: whether the generated Rust reproduces our wire quirks exactly (internally tagged enums with a `type` field, empty-braces variants like `BtScanStopped {}`, `#[serde(default)]` on `password`, the redacting `Secret` newtype, `Option` serialized as explicit `null` versus omitted), and enum and type naming churn against existing code. The three conversion layers (`WifiEvent` to `BridgeEvent`, `PairedDevice` to wire, `FailKind` to code) stay hand-written regardless, because they are domain-to-wire mapping, not types.

Is it worth it at this size? Split the question:
- **Spec as source of truth plus schema artifacts plus conformance tests: yes, worth it now.** About 45 messages across two daemons and one frontend, already one drift incident, and a user who wants clean separation. A TypeSpec file of a few hundred lines emits JSON Schema; Rust tests assert that every serialized event and every fixture command validates against it (a host-only `dev-dependency` such as `jsonschema`, which never enters the aarch64 binary); node tests do the same for the frontend fixtures. This catches drift without trusting any generator.
- **Generating Rust types from the spec: do a time-boxed spike first (about a day), adopt only if the generated code needs no hand patches.** If it does not, keep the serde types hand-written in `protocol/` (Pattern A step 3 makes that module self-contained and generation-ready) and rely on the conformance tests. That gives roughly 90 percent of the value with none of the build-pipeline risk, notably no Node toolchain inside the cross-compile and Yocto flow.
- **Generating JS types:** only valuable if the frontend adopts `@ts-check`; it has no build step and node is already used for its tests, so a committed `.d.ts` produced by a dev command is acceptable, but this is the frontend sibling's call.
- **Fallback if TypeSpec proves awkward:** Rust-first with `schemars` derive behind a dev-only feature, emitting JSON Schema from the real types, then the same TS and fixture steps. It is not spec-first, but it has no codegen risk on the Rust side and no drift on the Rust half. Pick by whether the user values the spec being the editing surface (TypeSpec) or minimal moving parts (schemars).

Sequencing with this refactor: do Pattern A steps 1 to 3 first (golden wire fixtures, lib split, `protocol/` module). Those fixtures become the seed for the spec's examples and the conformance corpus, and the spec work then targets a stable, isolated module instead of types tangled into `main.rs`. Any wire-visible cleanup (merged error events, request ids, snapshot) is a spec-level change made once in the spec, landed simultaneously in both daemons and the frontend, and versioned.
