# daemon/ (terminal-agent) architecture proposals

Scope: the `daemon/` crate, binary `terminal-agent`, a sync/threaded PTY-to-WebSocket relay on port 8766. Read-only study; every `main.rs:N` reference below is a line in `daemon/src/main.rs` (857 lines) as of this writing. Companion analyses: `controller/` and `frontend/` (see section 5 for what has to be coordinated).

## 0. Summary

857 lines in one file is not the problem. The problem is that the file mixes four different jobs (PTY syscalls, session lifecycle policy, client fanout, and a host-specific clock-sync feature that is 26 percent of the file), has zero tests, and contains several real defects that the mixing hides. The recommended path is a plain module split inside the one crate with a typed protocol module, a pure respawn policy, a generation counter on PTY lifecycle, bounded per-client queues and a thin `Pty` seam, delivered as pure-move commits that keep the build green. Keep threads, keep one crate, keep a separate process from `controller/`, do not adopt tokio, do not generate types from a spec yet. Five patterns are evaluated: module split (A), session actors with a registry (B), hexagonal ports and adapters (C), shared protocol crate or workspace (D), tokio rewrite (E).

## 1. Current architecture, measured

### 1.1 Line counts per logical section

| Lines | Count | Section |
|---|---|---|
| 1-27 | 27 | Crate doc comment (also the only in-code protocol spec; stale, see 1.7) |
| 28-61 | 34 | Imports and constants (port, respawn window, clock constants, hardcoded ssh key paths and user) |
| 63-89 | 27 | Config types and `load_config` |
| 91-195 | 105 | `Session` structs and `spawn_pty` (unsafe libc: openpty, dup, fcntl, setsid, TIOCSCTTY) |
| 197-338 | 142 | `impl Session`: new, write, resize, interrupt, clear, respawn, exit_code, record_involuntary_exit |
| 340-346 | 7 | `Clients` type and `broadcast` |
| 348-438 | 91 | `pump_session_output` and `handle_involuntary_exit` (read loop, auto-respawn policy) |
| 440-500 | 61 | `dispatch` (inbound message router) and `session_list_message` |
| 502-724 | 223 | Clock sync: `ClockStatus`, `ClockState`, host discovery (env, config, `ip route`, subnet probe), `probe_offset_ms` (ssh), `clock_sync_loop` |
| 726-799 | 74 | `handle_client` (handshake, reader loop, writer thread) |
| 801-857 | 57 | `main` (config, session spawn, threads, accept loop) |

Rough logical split: PTY and session mechanics 330 lines (38 percent), clock sync 223 (26 percent), transport and protocol 140 (16 percent), everything else (config, doc, wiring) 165. Tests: 0 (`grep -c '#[test]'` returns 0). CI runs `cargo build` and `cargo test` for the crate (`.github/workflows/ci.yml`), so the test step is currently a no-op beyond compiling.

### 1.2 Public surface

It is a binary; there is no library API. The effective external contract is:

- Port: TCP 8766 bound on `0.0.0.0` (main.rs:839, const at :45).
- Wire protocol: 3 inbound and 4 outbound JSON message types, documented only in prose (docs/terminal.md) and in the doc comment at main.rs:10-20. Parsed with `serde_json::Value` and `.get()` chains, not typed (main.rs:440-490), emitted with `json!` macros (main.rs:366, :402, :429, :495, :529).
- Env vars: `TERMINAL_AGENT_CONFIG` (main.rs:80), `TERMINAL_AGENT_VERMELHO_HOST` (main.rs:561).
- Config file keys: `sessions[].name`, `sessions[].command`, `vermelho_host` (main.rs:63-77). Shipped file has four identical `bash -l` sessions.
- Files read at runtime: the config, an ssh private key and known_hosts under `/var/lib/thing-revived/.ssh/` (main.rs:59-60). Child processes spawned: the configured commands, `ip`, `ssh`.
- Deployment: systemd unit sets the config path to `/etc/thing-revived/...` (unit file in build/overlay), while the compiled-in default is `/opt/thing-revived/...` (main.rs:44), a dead default that will confuse anyone running it by hand. `Restart=on-failure`, `RestartSec=2`, runs as root, `After=network.target` only. Logging is `println!`/`eprintln!` to journald.
- Static linking: `.cargo/config.toml` only sets linkers for armv7 and aarch64. `+crt-static` is injected via `RUSTFLAGS` in `build/build-image.sh:176`, so a bare `cargo build` is dynamic. That is fine but means nothing in the crate itself enforces the static contract; the build script's `file` check (build-image.sh:192) is the only guard.

### 1.3 Threads, channels, shared state

| Thread | Count | Created at | Lifetime |
|---|---|---|---|
| main (accept loop) | 1 | main.rs:846 | process |
| PTY reader per session (`pump_session_output`) | N | main.rs:822-826 | until child EOF; a new one is spawned on every respawn (:419, :484) |
| clock sync | 1 | main.rs:833-837 | process |
| client reader (the accepted connection's thread) | per client | main.rs:852 | until close |
| client writer | per client | main.rs:765 | until send failure, found lazily (:796-798), 10 s ping bounds the lag |
| delayed auto-respawn helper | transient | main.rs:412 | sleep then respawn then spawns a fresh pump |

Steady state with 4 sessions and one kiosk page: 1 + 4 + 1 + 2 = 8 threads. That is nothing for 488 MB; threads are not a memory problem here.

Channels: exactly one kind, `std::sync::mpsc::channel::<String>()` per client (main.rs:757), unbounded, with the `Sender` stored in `Clients = Arc<Mutex<Vec<Sender<String>>>>` (main.rs:340). `broadcast` locks the list, clones the serialized JSON per client and drops dead senders (main.rs:342-346). There is no per-client addressing at all: every message, including `session_list` and `clock_sync` on connect, goes to every client (main.rs:760, :763).

Shared state: `Arc<Vec<Arc<Session>>>`, each `Session` with `Mutex<SessionInner{master_fd, child}>` and `Mutex<Vec<Instant>>` for exit timestamps; `Arc<Mutex<ClockState>>`; the `Clients` mutex. Lock discipline is simple (short critical sections, fd copied out then used outside the lock), which is also the root of the lifecycle race in 1.6.

### 1.4 PTY lifecycle and session model

- Model: a flat, fixed `Vec<Session>` built once in `main` from config; session identity is the config index (main.rs:803-815). A session that fails to spawn is dropped from the vector, so index and position diverge, which `dispatch` correctly handles by searching on `s.index` (main.rs:446-453). The frontend's tabs come from `session_list`, so the dropped session simply vanishes; there is no "failed" state shown to the user.
- Spawn (main.rs:114-195): `openpty` with null termios and null winsize (so the PTY starts at 0x0 until the first `resize`), master marked `FD_CLOEXEC`, three stdio fds from the slave (two dups plus the original), `pre_exec` does `setsid()` then `ioctl(slave, TIOCSCTTY, 0)`.
- The SIGWINCH workaround is the `TIOCSCTTY` call and its comment (main.rs:169-185). Without a controlling terminal, `TIOCSWINSZ` on the master changes the size but the kernel has no foreground process group to signal, so ssh never forwards the new size and bash never redraws. It works, it is correctly commented, and it is the highest-value piece of knowledge in the file; any refactor must carry it and its comment verbatim, and it deserves a real-PTY regression test (see 1.8).
- Resize: `TIOCSWINSZ` on the master (main.rs:240-259). The frontend sends resize only for the active session on connect, resize and tab switch (frontend/app.js:151). With several clients the last resize wins, and there is no notion of "owning client".
- Interrupt: `kill(-pid, SIGINT)` to the process group created by `setsid` (main.rs:265-273). Clear: writes `0x0c` (main.rs:277).
- Exit handling: the reader thread sees EOF or EIO, then `handle_involuntary_exit` records the timestamp, broadcasts `exited`, and either schedules an auto-respawn (up to 3 in 30 s with delays 0, 2, 5 s) or flags `needs_manual_respawn` (main.rs:389-438). Manual `respawn` skips the limiter by design (main.rs:470-485).
- Scrollback: none in the agent. xterm.js owns it client-side. Output produced while no client is connected is read and discarded (the broadcast to an empty client list, main.rs:344). The shell survives a frontend reload (the process lives in the agent) but the screen does not: a reloaded page gets an empty terminal until the program redraws. "Sessions kept alive across reloads" is true for processes and false for visible state. This is the single most user-visible gap in the session model.
- Persistence: none beyond process lifetime. systemd's default `KillMode=control-group` means an agent restart kills every shell. docs/coming-soon.md lists tmux-style reattach as a wish; it cannot be done inside this process (see section 3, B and the merge discussion).

### 1.5 Wire protocol (port 8766)

Text frames, JSON, one object per frame, `type`-tagged. Outbound: `session_list`, `output` (PTY bytes as a lossy UTF-8 `String`), `exited`, `clock_sync`. Inbound: `input`, `resize`, `control` (actions `interrupt`, `clear`, `respawn`). Unknown or malformed inbound messages are silently ignored (main.rs:786-788, :444, :488). Binary and ping frames from the client are ignored (:791). There is no handshake version, no error message type, no ack, no sequence numbers, no per-session subscription. Frontend consumption is in frontend/app.js around line 245.

### 1.6 Smells and defects, with evidence

Ranked by consequence. Items marked (read, not reproduced) come from reading the code, not from a failing run.

1. Network exposure of a root shell. The server binds `0.0.0.0` (main.rs:839) and there is no auth and no Origin check, while docs/terminal.md justifies the lack of auth by calling it a loopback single-kiosk client. The code does not match the stated trust model. The device is reachable over the USB CDC-NCM link, so anything on that link can open a root shell. The same pattern exists in controller (controller/src/main.rs:921), which makes it a cross-cutting finding. The frontend only ever uses `ws://localhost:8766` (frontend/app.js:17), so a `127.0.0.1` bind is a one-line change unless a dev workflow depends on it (check docs/development.md and any dev-host usage before flipping; make it a config key with default loopback).
2. UTF-8 corruption at read boundaries: `String::from_utf8_lossy(&buf[..n])` on a 4096-byte read (main.rs:363). A multibyte sequence straddling two reads becomes U+FFFD twice. Visible with box-drawing or CJK output. Fix needs a carry-over of the incomplete tail. Binary output (`cat` of a binary) is inherently lossy under a JSON string transport; that is a protocol limitation, not a bug, but it should be a documented decision.
3. Manual respawn race (read, not reproduced). `respawn` kills the old group and closes the old fd (main.rs:288-297), installs the new fd, then the `control` handler spawns a second reader thread (main.rs:482-484). The old reader thread is still alive: it re-reads `master_fd` from the lock on every iteration (main.rs:357), so it will pick up the new fd and read concurrently with the new reader, interleaving or splitting chunks. Separately, the old reader can hit EIO from the killed child and call `handle_involuntary_exit` (main.rs:374), which counts a user-initiated respawn as an involuntary exit and can trigger a third spawn, killing the fresh child. Also an fd number copied out of the lock (main.rs:357-361) can be closed and reused by another session's fd before the `read`. The cure is a generation id: each spawn gets a number, the reader carries its generation, and anything from a stale generation is dropped.
4. Zombie children (read, not reproduced). `inner.child = None` (main.rs:285) drops a `Child` without `wait`, and nothing reaps after `SIGKILL`; `exit_code` only calls `try_wait` once, right after EOF, which can return `None` if the child is not yet reapable. Repeated respawns leak zombies until the agent restarts.
5. Unbounded per-client queue: `channel::<String>()` (main.rs:757). A stalled client (page frozen, TCP window full) makes the writer thread block in `send` while PTY output keeps accumulating in memory at full speed (`yes` in a shell is enough). On a 488 MB device with no swap this ends in the OOM killer taking out the agent or something more important.
6. Fanout is global where it should be targeted. `session_list` and `clock_sync` are broadcast to every client on every connect (main.rs:760, :763). Harmless with one client, wrong as soon as there are two.
7. No handshake timeout and no connection cap: `tungstenite::accept` blocks on a bare thread per connection (main.rs:748, :852). A port scanner on the bound interface can hold threads open indefinitely.
8. Panic-based config errors (main.rs:81-87) are acceptable fail-fast under `Restart=on-failure` but produce an infinite 2 s crash loop on a bad config with no distinct exit code or message structure. Mutex `.unwrap()` everywhere (for example main.rs:225, :241, :266, :344): one poisoned lock cascades panics through every thread. A panicking reader thread dies silently and that session is deaf with nothing supervising it; `profile.release` has no `panic = "abort"`, so the process stays up in the degraded state instead of letting systemd restart it.
9. Clock sync does not belong to a PTY relay. It is 223 lines (main.rs:502-724) of host discovery by parsing `ip` output, subnet probing, and an ssh round trip to a hardcoded user on the dev workstation (`andremmfaria`, main.rs:61; key path :59), run forever every 60 s even with zero clients. It is in this binary only because the terminal-agent happened to be the process that already had a broadcast channel to the frontend. Its parsing and offset math are the most unit-testable code in the file and are currently untestable because they are fused to `Command` and `TcpStream` calls (main.rs:578, :599, :627, :645).
10. Stale text: the crate doc says `terminal/frontend/` and compares to `bridge/` (main.rs:6, :22-26), and Cargo.toml comments say "same choice bridge/ makes" and mention gnueabihf. Cosmetic but it misleads new readers; fix inside whichever commit moves that code.
11. Dependency note: `tungstenite` is 0.30 here and 0.24 in `controller/Cargo.toml`. The lock file has 35 packages, mostly pulled by tungstenite (http, httparse, sha1, rand 0.10 with chacha20, data-encoding, bytes, log). The release aarch64 binary is 1,518,456 bytes (measured from `daemon/target/aarch64-unknown-linux-gnu/release/terminal-agent`), armv7 1,120,440 bytes. Minimal-dependency preference is already well served; a hand-rolled RFC 6455 server would save perhaps 150 to 250 KB (estimate) and cost correctness, so it is not worth considering.

### 1.7 Is 857 lines in one file actually a problem

No, as a size. The sibling `controller/src/main.rs` is 1009 lines with 4 more modules totaling 2,497. A single 857-line file with 11 clearly commented sections is readable top to bottom in one sitting, and a single file has real advantages for a one-person project: grep-friendly, no module plumbing. What is a problem is the shape: no seams for tests (`Session::new` spawns a real PTY, so even `dispatch` and `session_list_message` cannot be constructed without a child process), unsafe fd code interleaved with business policy, a second unrelated feature sharing the file and threads, and an in-process protocol that exists only as a comment. The split pays for itself through testability and isolation, not through line count. If the clock code were deleted tomorrow the remaining 630 lines could stay in one file without much pain.

### 1.8 What is unit-testable today versus what needs a real PTY

Testable today without refactor, in a `#[cfg(test)] mod` in the same file: `ClockStatus::as_str`, `clock_sync_message` (needs only a `Mutex<ClockState>`), `record_involuntary_exit` (a `Session` can be built by struct literal inside the module with `master_fd: None`), the offset arithmetic if extracted from `probe_offset_ms`. Practically nothing else, because `dispatch`, `session_list_message` and `handle_involuntary_exit` want `Arc<Session>` objects and `Session::new` spawns children.

Testable after small extractions: JSON message encode and decode (typed enums), `ip route` and `ip addr` parsing (pure functions taking `&str`), subnet candidate enumeration (pure over u32 math, main.rs:612-626), the respawn policy (pure struct taking `now: Instant`; today `Instant::now()` is called inside the method at main.rs:332), config validation, UTF-8 reassembly.

Needs a real PTY: the `TIOCSCTTY` and SIGWINCH behavior, process-group `SIGINT`, EOF versus EIO on child exit, fd inheritance (`FD_CLOEXEC`), zombie reaping. These can still run in ordinary CI on `ubuntu-latest` because `openpty` works there (it is plain Linux, not Car Thing specific): spawn `sh -c 'trap "stty size" WINCH; echo ready; while :; do sleep 1; done'`, resize, assert output. A fake PTY cannot test any of those; it only helps with policy and routing logic. This matters for the hexagonal option (pattern C).

## 2. Likely feature pressure

From docs/coming-soon.md, docs/terminal.md known limitations, and the premise given:

| Feature | Pressure on daemon/ | Needs |
|---|---|---|
| More sessions, dynamic add or remove | High and cheap | A registry instead of a frozen `Vec`; stable session ids instead of config indices |
| Reload-safe screen (scrollback replay) | High; user-visible today | Per-session ring buffer (for example 64 KB each, 4 sessions is 256 KB) plus an attach message that replays it; per-client addressing |
| Session persistence across agent restart or reboot | Medium; listed in docs/coming-soon.md | Cannot live in this process. Real options: delegate to `tmux` or `abduco` as the session command (config only, zero code), or split a tiny session-host process (dtach style) from the relay. Reboot persistence of shell state is not feasible without something like tmux-resurrect and should be dropped from expectations |
| Auth | Medium; becomes mandatory if the listen address ever leaves loopback | Bind address config, optional shared-token query param, Origin policy; note frontend is `file://` served so the Origin header is likely `null` in Chromium (verify on device), which makes Origin checks near useless; loopback bind is the real control |
| Wedge detection / auto-respawn improvements | Medium | A per-session supervisor with a liveness notion; needs generation ids anyway |
| Per-tab activity indicator | Low in daemon | Already derivable client-side from `output` messages |
| Shared protocol with controller | Medium | See sections 5 and 3D |
| Health endpoint | Low-medium | A tiny read-only request type on a second port or a `status` message; needs session state to be queryable, which a registry gives |
| Merge with controller | Evaluated below | Decided against |

### 2.1 The merge question (daemon and controller as one process)

Honest answer: keep them separate. The arguments:

- RAM: the saving is real but tiny. The two static binaries are about 1.5 MB on disk each; resident cost per process for this kind of workload is plausibly 2 to 6 MB (estimate; measure with `systemctl status` or `/proc/<pid>/status` before citing it). Merging saves one process's fixed overhead, maybe a few MB out of about 488 MB total, and the controller's `zbus` async-io executor threads stay either way. RAM is not the constraint that decides this.
- Supervision and blast radius are the decisive axes. `terminal-agent` holds the user's live shells and ssh sessions; its restart destroys them (default `KillMode`). `controller-agent` does `EVIOCGRAB` on input devices, talks D-Bus to BlueZ, drives wpa_supplicant and changes backlight and timezone; it is the more likely of the two to hit weird hardware and driver states, and it has 3,500 lines of code against the terminal agent's 857. Merged, any controller panic, D-Bus hang or wifi bug kills every shell; today it does not. Separate units also give independent `Restart=` behavior and independent journald streams.
- Privilege separation is available for free if ever wanted: the terminal agent needs no evdev, no D-Bus, no network control; the controller needs no PTYs. Merging forces the union of capabilities into one root process.
- Coupling that does exist is only at the frontend, which already speaks to both over two WebSockets, and that is a healthy seam.
- Where merging would win: a single listening port and one auth mechanism, a single place to generate `clock_sync`, one config. All three are achievable with a shared protocol definition and shared constants without sharing a process.

The thing worth moving is not the whole agent but the clock-sync feature: it is host-specific, has nothing to do with PTYs, and logically sits next to the controller's timezone and settings code. Moving it changes who sends `clock_sync` (port 8765 instead of 8766), so it is a frontend and controller coordination item (section 5), not a daemon-only decision. In this proposal it is first isolated into a `clock/` module with no behavioral change; relocation is a separate decision.

## 3. Patterns

### 3A. Module split by responsibility, one crate (threads kept)

Idea: pure moves first, then fix defects behind the new module boundaries. No new concepts beyond a typed protocol and a pure respawn policy.

Target tree (all under `daemon/`):

```
daemon/
  Cargo.toml                      unchanged deps; add [lib] so tests/ can import, see below
  .cargo/config.toml              unchanged
  terminal-agent-config.json      unchanged format, new optional keys: listen, scrollback_bytes
  src/
    main.rs                       ~40 lines: parse env, load config, build registry, start threads, accept loop
    lib.rs                        declares modules; lets tests/ integration tests use the crate
    config.rs                     ConfigFile, SessionConfig, load() -> Result<Config, ConfigError>, env path resolution, validation
    protocol.rs                   ClientMsg and ServerMsg serde enums, ControlAction, ClockStatus wire form, encode/decode, golden-test target
    pty.rs                        ALL unsafe libc: openpty, spawn with setsid+TIOCSCTTY, read, write_all, set_winsize, killpg, close; PtyMaster with Drop
    session.rs                    Session: owns PtyMaster + Child, generation counter, interrupt/clear/respawn, reaping, exit code
    respawn_policy.rs             pure RespawnPolicy { window, max, delays }: on_exit(now) -> Decision::{Retry(delay), GiveUp}
    hub.rs                        ClientHub: bounded per-client queues, broadcast, send_to, drop-slow-client policy
    server.rs                     accept loop, handshake with timeout, client reader/writer threads, dispatch(ClientMsg) -> Session calls
    clock/
      mod.rs                      ClockState, ClockStatus, clock_sync_loop (thread body), message construction
      discovery.rs                discover_vermelho_host: env, config, default route, subnet probe; pure parsers take &str
      probe.rs                    probe_offset_ms: ssh invocation and RTT-corrected offset math (pure fn over timestamps)
  tests/
    pty_roundtrip.rs              real PTY on Linux CI: echo round trip, resize delivers SIGWINCH, interrupt kills group, respawn gives fresh output
    protocol_golden.rs            JSON fixtures decode and re-encode byte-stable (shared fixtures dir, see section 5)
```

Mapping from current code:

| Current lines | Destination |
|---|---|
| 1-27 doc comment | `protocol.rs` module doc plus `main.rs` short header; update stale names |
| 28-61 constants | split: `config.rs` (port, default path), `respawn_policy.rs` (window, delays), `clock/` (interval, backoff, ssh paths) |
| 63-89 | `config.rs` |
| 91-195 spawn_pty and structs | `pty.rs` (spawn, fd handling), `session.rs` (structs) |
| 197-338 impl Session | `session.rs` with raw syscalls replaced by `PtyMaster` methods; `record_involuntary_exit` becomes `respawn_policy.rs` |
| 340-346 | `hub.rs` |
| 348-438 | reader loop in `session.rs`, policy in `respawn_policy.rs`, orchestration in `session.rs` |
| 440-500 | `server.rs` dispatch on typed `ClientMsg`; session_list builder in `protocol.rs` plus registry |
| 502-724 | `clock/` |
| 726-799 | `server.rs` |
| 801-857 | `main.rs` |

Key types and flow:

```rust
// protocol.rs
#[derive(Deserialize)] #[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Input { session: usize, data: String },
    Resize { session: usize, cols: u16, rows: u16 },
    Control { session: usize, action: ControlAction },
}
#[derive(Serialize)] #[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    SessionList { sessions: Vec<SessionInfo> },
    Output { session: usize, data: String },
    Exited { session: usize, code: Option<i32>, auto_respawning: bool, needs_manual_respawn: bool },
    ClockSync { offset_ms: i64, status: ClockStatus, #[serde(skip_serializing_if = "Option::is_none")] age_s: Option<u64> },
}
```

This mirrors how `controller/` already models its protocol (tagged serde enums with unit tests, controller/src/main.rs:80, :284, :941-1004), so it is consistent with the sibling and should reduce, not add, surprise.

Flow: client frame -> `server::handle_client` reader thread -> `protocol::decode` -> `dispatch` -> `Session` method -> `pty.rs`. Reverse: per-session reader thread -> `UTF-8 reassembler` -> `ServerMsg::Output` -> `hub.broadcast` -> bounded queue per client -> writer thread -> socket.

Concurrency model: unchanged (threads plus mutexes plus mpsc), with three targeted fixes folded in: generation counter on `Session` so a reader only acts while its generation is current (fixes smell 3), `sync_channel` with `try_send` and drop-on-full in `hub.rs` (fixes smell 5), and `Child::wait` after kill (smell 4).

Testing strategy: pure unit tests per module (protocol golden, policy table tests with injected `now`, discovery parsers over captured `ip` output, offset math, UTF-8 reassembler with split multibyte inputs, config validation errors); real-PTY integration tests in `tests/` run on CI Linux. No fake PTY needed.

Dependency and size impact: zero new dependencies; `lib.rs` plus `main.rs` costs nothing at link time (LTO and `opt-level = "z"` already on). Expect the binary within a few KB of today's 1.5 MB, plausibly slightly smaller because `serde_json::Value` pointer-chasing is replaced by derived deserialization (estimate, measure after step 3).

Pros: smallest conceptual change, every step is mechanically verifiable, works with the 35-package lock file, directly mirrors the controller's style, creates the seams that every other pattern needs anyway. Cons: does not by itself change the concurrency design; the lifecycle fixes are small behavior changes needing the real-PTY tests first; `lib.rs` plus `main.rs` doubles the crate roots (a standard Rust idiom, but a new thing in this repo).

Migration, each step is its own commit, `cargo build` and `cargo test` green, plus a device smoke test at the marked steps:

1. Add `#[cfg(test)]` characterization tests in `main.rs` for the pure bits that exist today (`clock_sync_message`, `record_involuntary_exit`). Also fix stale comments. S.
2. Move clock code to `clock/` verbatim (pure move, no signature changes beyond `pub(crate)`). Add parser tests by extracting `&str`-taking helpers in the same commit or a follow-up. S.
3. Introduce `protocol.rs` typed enums with golden JSON tests asserting the JSON shapes match the current output byte for byte (including field order not mattering, and `age_s` omitted when `ok`). Switch `dispatch` and emitters over. Keep `Value` fallback for unknown types (ignored, as now). S to M. Smoke test on device.
4. Extract `config.rs` returning `Result`; `main` converts error to message plus `exit(78)` or similar. Add `listen` key defaulting to `127.0.0.1` (behavior change, called out in the commit message). S. Smoke test on device.
5. Extract `pty.rs` (all `unsafe`) with `PtyMaster` that owns the fd and closes on `Drop`; `session.rs` uses it. Port the `TIOCSCTTY` comment verbatim. Add `tests/pty_roundtrip.rs` including the SIGWINCH test before touching behavior. M.
6. Extract `respawn_policy.rs` as a pure type with table-driven tests. S.
7. Add generation counter and reaping to `session.rs`; manual respawn no longer double-reads or counts as involuntary. Regression test: respawn twice, assert one output stream and `exited` counts. M, the riskiest behavior step. Smoke test on device.
8. `hub.rs` with bounded queues and targeted `send_to` for `session_list` and `clock_sync` on connect. S to M.
9. `server.rs` split, handshake timeout, `UTF-8 reassembler` in the output path. S to M.

Effort: M overall (roughly 2 to 4 focused days, mostly steps 5 to 7). Risk: low for steps 1 to 6 (moves), medium for 7 to 9 (behavior).

### 3B. Session actors with a registry and supervisor

Idea: one owner per session, communicating only by messages; a `Registry` maps session ids to actor handles; a supervisor decides respawn. This is the architecture you would want if sessions become dynamic, if persistence becomes a split session-host, and if wedge detection arrives.

Target tree:

```
daemon/src/
  main.rs                         wiring
  lib.rs
  config.rs                       as in A
  protocol.rs                     as in A, plus Attach{session,replay:bool} and Detach later
  pty.rs                          as in A (unsafe lives only here)
  actor/
    mod.rs                        SessionHandle (cloneable Sender<SessionCmd>), SessionId, SessionEvent
    session_actor.rs              thread owning PtyMaster+Child+scrollback ring; loop over SessionCmd and reader events; generation-tagged
    reader.rs                     per-generation blocking read thread, forwards ReaderEvent{gen, Bytes|Eof} to the actor
    scrollback.rs                 bounded ring buffer of raw bytes, replay snapshot
    supervisor.rs                 owns RespawnPolicy per session; reacts to SessionEvent::Exited; issues Respawn after delay (timer thread or a single sleeper)
  registry.rs                     SessionId -> SessionHandle; add/remove; list for session_list
  hub.rs                          as in A: bounded per-client queues, ClientId addressing
  server.rs                       transport only: decode, route ClientMsg to registry, attach clients
  clock/ ...                      as in A, or removed
```

Mapping: `Session` struct and `impl Session` become `session_actor.rs`; `pump_session_output` becomes `reader.rs`; `handle_involuntary_exit` becomes `supervisor.rs`; `dispatch` becomes `registry.send(id, SessionCmd)`; `Clients` becomes `hub.rs` as in A.

Key types and message flow:

```rust
enum SessionCmd { Input(Vec<u8>), Resize{cols:u16,rows:u16}, Interrupt, Clear, Respawn{manual:bool}, Attach(ClientId, SyncSender<Arc<str>>), Detach(ClientId), Shutdown }
enum ReaderEvent { Bytes{gen:u64, data:Vec<u8>}, Eof{gen:u64} }
enum SessionEvent { Output{session:SessionId, bytes:Vec<u8>}, Exited{session:SessionId, code:Option<i32>} }
```

Client frame -> server reader thread -> `registry.get(id).send(SessionCmd)` -> actor thread. PTY -> `reader` thread (blocking `read`) -> `ReaderEvent` over mpsc -> actor, which ignores any event whose `gen` differs from its current generation (this makes the respawn race structurally impossible), appends to the scrollback ring, and pushes `ServerMsg::Output` to attached clients via `hub`. Exit -> actor emits `SessionEvent::Exited` to the supervisor, which consults the pure policy and sends `Respawn` back.

Concurrency model: per session, 1 actor thread plus 1 reader thread per generation (the reader exits on EOF and a new one starts at respawn); no mutex around fds because only the actor touches them. A single-thread-per-session design using `poll(2)` over the master fd and a self-pipe was considered and rejected: it is a hand-written reactor, more unsafe code, more subtle than two threads, and the thread cost is trivial here.

Testing strategy: the actor is driven entirely through its `SessionCmd` and `ReaderEvent` channels, so lifecycle tests (stale generation dropped, exit during respawn, scrollback replay on attach) need no PTY if the reader is injected as a trait object or a channel; real-PTY tests as in A for the syscall layer.

Dependency and size impact: none; roughly +300 lines of code overall (estimate) because of the command and event enums.

Pros: removes shared mutable session state and its race class; scrollback replay and per-client attach fall out naturally; the supervisor gives a place for wedge detection; best fit for dynamic sessions and for the later session-host split. Cons: more types and more indirection than 4 to 8 sessions justify today; thread count rises (8 to about 12 in steady state, immaterial); the whole design has to be understood by one maintainer who currently does not need it; and every behavior is now asynchronous, so errors such as "write to a dead session" must be reported through events rather than return values.

Migration: do A steps 1 to 6 first (it is a superset). Then: (1) introduce generation counter inside the existing `Session` (A7), (2) move `pump_session_output` into `reader.rs` sending events instead of calling `broadcast`, (3) put the `Session` behind a command channel and a thread, (4) add `scrollback.rs` and `Attach`, (5) extract `supervisor.rs`, (6) switch `dispatch` to `registry`. Each step is behind the existing JSON protocol, so the frontend is untouched until `Attach` is added (additive message). Effort: L (including A's work, M to L on top of A). Risk: medium, because it rewrites the one piece of concurrent code that currently works acceptably on hardware.

### 3C. Hexagonal: domain core with port traits and adapters

Idea: a pure domain core (sessions, respawn policy, routing) behind port traits `PtyPort`, `ClockSource`, `Transport`, and `Config`; adapters for the real PTY, real ssh, real WebSocket; fakes for tests. Included because it was requested as a candidate and because it is the right answer for larger systems. Candid verdict: it is overkill at this size in its full form.

Target tree:

```
daemon/src/
  main.rs                         composition root: builds real adapters, injects into core
  lib.rs
  domain/
    mod.rs
    session.rs                    Session state machine: Running{gen} | Exited | CrashLooping; pure transitions on events
    respawn_policy.rs             pure
    routing.rs                    ClientMsg -> Vec<Effect>
    effects.rs                    Effect::{WritePty, ResizePty, SignalGroup, Respawn, Emit(ServerMsg), ScheduleAfter}
  ports/
    pty.rs                        trait PtyPort { spawn(&Cmd)->Result<Box<dyn PtyHandle>>; } trait PtyHandle { read, write_all, set_winsize, kill_group, try_wait }
    clock.rs                      trait HostProbe { discover()->Option<Host>; offset(&Host)->Option<i64> }
    transport.rs                  trait ClientTransport { accept()->Box<dyn ClientConn>; }
  adapters/
    pty_libc.rs                   unsafe libc implementation incl. TIOCSCTTY
    pty_fake.rs                   in-memory pair of channels, scripted exits (cfg(test) or dev-dependency)
    ws_tungstenite.rs             handshake, reader/writer threads
    probe_ssh.rs                  ip+ssh implementation
    probe_fake.rs
  protocol.rs
  config.rs
  hub.rs
```

Mapping: `spawn_pty` becomes `adapters/pty_libc.rs`; `impl Session` becomes `domain/session.rs` calling the port; `dispatch` becomes `domain/routing.rs` returning effects; clock functions split between `ports/clock.rs` and `adapters/probe_ssh.rs`; `handle_client` becomes `adapters/ws_tungstenite.rs`.

Flow: transport adapter decodes a frame, the core turns `(state, event)` into `(state', effects)`, a small interpreter executes effects through the ports. This is the functional-core style: the domain is deterministic and exhaustively testable.

Concurrency model: unchanged threads at the adapters; the core is single-threaded behind a mutex or a dedicated core thread receiving events (which effectively converts into B).

Testing strategy: domain tests use `pty_fake` with scripted exits, deterministic time injected through the effect `ScheduleAfter`; real PTY still needed for `pty_libc` and the TIOCSCTTY behavior, which a fake structurally cannot cover (and that is the one thing that has actually broken on this project, per the comment at main.rs:174-182).

Dependency and size impact: zero new crates; trait objects add negligible code. Maintenance cost is the real impact: roughly 4 more files per concept.

Pros: strongest testability, clean seams for Linux-host development without a device, clear home for future transports (for example a Unix socket for a local CLI). Cons: for a relay whose core logic is about 150 lines (policy plus routing) it buys indirection and ceremony disproportionate to its value; the fake PTY gives false confidence on the hardest-to-get-right part; two layers of types (domain events and wire messages) must be kept in sync; the effort is mostly spent on plumbing rather than fixing the defects in 1.6.

Migration: after A steps 1 to 6, introduce `PtyHandle` trait around `PtyMaster` only (a single trait, about 30 lines), then optionally a `HostProbe` trait. Stop there; that is "A plus two traits", which is the part of C worth having. The full domain and effects interpreter is not recommended. Effort: M for the thin version, L for the full version. Risk: low for the thin version, medium for the full version (large diff, easy to subtly change timing of respawn).

### 3D. Shared protocol crate (path dependency or Cargo workspace)

Idea: one crate `protocol/` (or `thing-protocol`) holds the serde types for both WebSocket protocols, used by `daemon/` and `controller/`; optionally a Cargo workspace at repo root.

Target tree (path-dependency variant, "D-lite", minimal disruption):

```
protocol/
  Cargo.toml                      serde, serde_json only, no_std not required
  src/
    lib.rs
    terminal.rs                   ClientMsg, ServerMsg for port 8766 (the types from 3A protocol.rs)
    controller.rs                 controller's ClientCommand and event enums (owned by the controller analysis)
    clock.rs                      ClockStatus and clock_sync payload, used by whichever process ends up sending it
  fixtures/
    terminal/*.json               golden messages, one per variant
    controller/*.json
daemon/Cargo.toml                 protocol = { path = "../protocol" }
controller/Cargo.toml             protocol = { path = "../protocol" }
```

Workspace variant ("D-full"): add a root `Cargo.toml` with `members = ["protocol","daemon","controller"]`; one `Cargo.lock`, one `target/`.

Impact to check before choosing the workspace variant (all verified by reading the repo): CI jobs use `working-directory: daemon` and cache `daemon/target` keyed on `daemon/Cargo.lock` (ci.yml); `build/build-image.sh:173-176` runs `cargo build` inside each crate directory and expects the artifact under that crate's `target/`; `.cargo/config.toml` lives in `daemon/` only. A workspace moves all of that. The path-dependency variant keeps separate locks and targets, so none of those change, which is why D-lite is the only form worth considering now. A version unification side effect: both crates would resolve serde and serde_json together, but tungstenite stays at 0.30 and 0.24 respectively until someone bumps the controller (a separate, unrelated task).

Concurrency model: n/a. Testing: serde round-trip tests live in `protocol/` once; each consumer only tests behavior.

Dependency and size impact: no new third-party crates; the daemon would compile in controller-only types only if they were in the same crate with no feature gating, so gate with cargo features (`terminal`, `controller`) or keep modules unused (dead code is removed by LTO, but compile time and review noise grow).

Pros: single source of truth in Rust; drift between the daemon and controller messages (for example the `clock_sync` type if it moves) becomes a compile error. Cons: only pays when there is a second Rust consumer of the same types. Today the daemon's protocol types are used by exactly one Rust binary; the controller's by one. The frontend is the other side of both protocols, and a Rust crate does nothing for it. Premature unless `clock_sync` moves or the two protocols converge. It also forces coordinated edits across crates that currently build and release independently.

Migration: only after A step 3 exists and only when a second Rust consumer appears. Then: move `protocol.rs` to `protocol/src/terminal.rs` verbatim, add the path dependency, keep CI as is plus add `protocol/` tests. Effort: S for D-lite, M for D-full. Risk: low for D-lite, medium for D-full (CI, build script, cache keys).

### 3E. Async rewrite on tokio versus staying on threads

Idea: tokio (current-thread runtime) plus `tokio-tungstenite`; each PTY master as `AsyncFd`; sessions as tasks; `select!` for client, PTY and timers.

Target tree: as in A or B with `server.rs`, `session.rs` and `clock/` rewritten as async; `pty.rs` gains `AsyncFd` wrappers and non-blocking fd setup.

Honest cost and benefit for this device:

- Memory: tokio itself is not heavy; a current-thread runtime with a handful of tasks is a few hundred KB resident (estimate). Thread-per-connection with 8 threads costs a few tens of KB each of resident stack (virtual stack reservation is large but untouched pages are not resident). On 488 MB neither approach is a RAM problem. Do not choose either for RAM reasons.
- Binary size: tokio features `rt`, `net`, `io-util`, `process`, `sync`, `time`, `macros`, plus `tokio-tungstenite`, `futures-util`, `mio`, `socket2`, `pin-project-lite`, `signal-hook-registry` and their transitive set would likely add several hundred KB to roughly a megabyte to the 1.5 MB binary and move the lock file from 35 packages to roughly 45 to 55 (estimates, unverified). That is a direct hit on the "minimal dependencies preferred" requirement.
- Consistency: both repo siblings are sync and threaded, and docs/terminal.md states that as a deliberate choice; `controller/` already carries `zbus` with its own async-io executor, so introducing tokio in one process and async-io in another adds a second async stack to maintain across the repo.
- Real benefits: cancellation and timeouts (handshake, write stalls) become one-liners; bounded channels and `select!` make the actor pattern pleasant; no thread-per-connection. These are real, but every defect in 1.6 is fixable in the threaded model with bounded `sync_channel`, socket timeouts and a generation counter, at far lower risk.
- Hidden costs: PTY masters need `O_NONBLOCK` plus `AsyncFd` (or `spawn_blocking`, which defeats the purpose); the `pre_exec` TIOCSCTTY logic keeps working through `std::process::Command`, but `tokio::process::Command` wraps it with its own SIGCHLD handling, so the one hard-won piece of knowledge must be re-verified on hardware; the whole code base would need a rewrite for no user-visible gain.

Pros: ergonomics for future concurrent features; scales to many sessions and clients. Cons: all of the above; also a rewrite of working, hardware-verified code.

Migration (if ever chosen): keep `pty.rs` and `protocol.rs` and `respawn_policy.rs` from A (they are runtime agnostic), rewrite only `server.rs`, `session.rs` and `clock/`. This is the strongest argument for doing A first: it makes a later async rewrite a 3-file job instead of a total rewrite. Effort: L. Risk: high, because failure modes appear only on hardware (the SIGWINCH comment is the precedent).

## 4. Comparison and recommendation

Scores: 1 is poor, 5 is strong. "Defects fixed" is how many of the 1.6 items the pattern naturally addresses.

| Criterion | A: module split | B: actors | C: hexagonal (full) | D: shared crate (lite) | E: tokio |
|---|---|---|---|---|---|
| Effort | M | L | L | S to M | L |
| Risk to working hardware behavior | Low (moves) to medium (lifecycle fix) | Medium | Medium | Low | High |
| Defects fixed (1.6) | 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 as follow-on steps | 3, 4, 5, 6, plus scrollback | Same as A once built, with more ceremony | None | 5, 7 easier, rest equal |
| Testability gain | 4 | 4 | 5 for logic, still 0 for ctty | 3 (protocol only) | 3 |
| Binary size delta | about 0 | about 0 | about 0 | about 0 | +0.3 to 1 MB (estimate) |
| New dependencies | 0 | 0 | 0 | 0 | about 10 to 20 packages (estimate) |
| Fits more sessions / dynamic sessions | 3 | 5 | 4 | n/a | 5 |
| Fits scrollback replay and attach | 3 (needs hub work) | 5 | 4 | n/a | 4 |
| Fits persistence (session host) | 2 | 4 | 3 | n/a | 3 |
| Fits auth | 4 | 4 | 4 | n/a | 4 |
| Fits shared protocol with controller | 4 (protocol.rs lifts out cleanly) | 4 | 4 | 5 | 3 |
| Consistency with controller style | 5 | 3 | 2 | 4 | 1 |
| Cognitive load for one maintainer | Low | Medium | High | Low | Medium |

### Recommendation

Adopt pattern A, with three borrowed ideas, in this order: the typed `protocol.rs`, a pure `respawn_policy.rs`, and a generation counter plus bounded per-client queues (the cheap, high-value 20 percent of B), and with a single `PtyHandle` trait only if and when a policy test actually needs it (the cheap slice of C). Isolate `clock/` as a module now. Treat D-lite as a trigger-based follow-up when `clock_sync` relocates or the controller protocol is next refactored. Treat full B as a future step that A leaves room for, to be taken only when sessions become dynamic or persistence (as a split session-host) is scheduled.

Reasoning chain, each tied to a property of this problem rather than general best practice: (1) the defects that matter (root shell on `0.0.0.0`, respawn race, lossy UTF-8, unbounded queue, zombies) are each local and fixable inside modules, so structure should be chosen to make those fixes testable, and A with a real-PTY integration test is exactly that; (2) the one piece that has actually broken on hardware (controlling terminal and SIGWINCH) cannot be covered by a fake, which removes the main argument for C; (3) 4 to 8 sessions and 1 to 2 clients make thread-per-connection free, which removes the main argument for E; (4) the device is memory-bound but not at this scale, and the user's dependency preference is already met, so any pattern that adds crates must earn it, and none of the structural patterns do; (5) B's value is real but conditional on features (dynamic sessions, replay, wedge detection) not yet scheduled, so it is the right second step, not the first.

Steelman of the alternative: "go straight to B, because you will need replay and a supervisor soon and doing A then B means touching the same code twice." That is a fair argument for the lifecycle code (session.rs). The mitigation in this plan is that A's steps 5 to 7 already put the generation counter and the policy where B wants them, so the second touch is moving code between threads, not rewriting it. If scrollback replay is the next feature scheduled, fold B's scrollback ring and `Attach` into A step 8 and skip the intermediate form.

What would change this recommendation: a requirement for hundreds of concurrent sessions or multi-client collaboration (then E becomes defensible); a decision to split a session-host process for persistence (then B's actor boundary becomes the process boundary); the controller and daemon protocols converging into one (then D-full); evidence that the agent's resident memory is much larger than expected on device (then re-measure before choosing anything).

### What NOT to do

- Do not merge `terminal-agent` into `controller-agent` (section 2.1): saves a few MB at the cost of coupling live shells to Bluetooth, Wi-Fi and evdev code.
- Do not rewrite on tokio now: the cost lands on dependency count, binary size and hardware re-verification; the benefit lands on problems this device does not have.
- Do not build the full hexagonal domain and effects interpreter: 150 lines of logic do not justify a second type system, and the fake PTY would test everything except the thing that breaks.
- Do not move to a Cargo workspace just to share types: it changes CI cache keys, `working-directory`, `build-image.sh` artifact paths and the lock policy for two crates that currently ship independently.
- Do not change the PTY spawn sequence while refactoring: `setsid` then `TIOCSCTTY` on the slave inside `pre_exec`, with the slave fd passed by raw integer, must move byte-for-byte with its comment, and the real-PTY SIGWINCH test must exist before the move.
- Do not "fix" persistence by adding scrollback to disk or by serializing PTY state; if reattach across agent restarts is wanted, run `tmux` or `abduco` as the session command (config change only) or build a separate session-host process.
- Do not hand-roll WebSocket framing to save dependencies: the saving is a few hundred KB at best and the risk is protocol bugs on a root shell endpoint.
- Do not treat Origin checking as the auth story: the kiosk page is `file://` served and the Origin is likely `null` there (verify on device); loopback binding is the real control, with an optional token for any future non-loopback use.

## 5. Shared concerns for the controller/ and frontend/ analyses

1. The wire protocol is the actual shared artifact. Daemon: 7 message types plus `ControlAction`. Controller: a much larger set (button, knob, bt_*, wifi_*, brightness_*, timezone_*; see controller/src/main.rs:80 and :284). Frontend: hand-written JS consuming both (`frontend/app.js:245` for the daemon, `connectBridge` onward for the controller). All three analyses should agree on: tagged-union envelope with `type` (already universal), snake_case names (already universal), a single naming scheme for the session identifier (today the daemon uses `index` in `session_list` and `session` elsewhere; if dynamic sessions arrive, use a stable `id`), an error message type (neither side has one), and an optional `hello` with `protocol_version` so a stale frontend page and a new agent can detect mismatch. Change these additively; the frontend is baked into the image together with the agents, so lockstep releases mean strict versioning is a nicety, not a necessity.
2. `clock_sync` ownership. It currently rides on 8766, produced by the terminal agent, consumed by the frontend clock UI. It conceptually belongs with controller's timezone and settings code. Moving it needs a controller decision (does it want an ssh-to-dev-host feature at all, given the hardcoded user and key path) and a frontend change (subscribe on the bridge socket). Until decided, the daemon keeps it in an isolated `clock/` module so the move is a copy.
3. Bind address and auth must be one policy across both agents: both bind `0.0.0.0` today (main.rs:839 and controller/src/main.rs:921). Recommend a shared convention: config key `listen` defaulting to `127.0.0.1`, same name, same semantics, documented once in docs/terminal.md or a new shared doc. The controller side is the larger exposure (it can change Wi-Fi credentials and Bluetooth state).
4. Fixtures. Whatever is decided on spec tooling, put canonical JSON examples for every message in one directory (for example `protocol/fixtures/`), one file per variant, and make three tests consume them: the Rust daemon's serde round-trip, the controller's, and a node test in the frontend (CI already has a node job). That gives drift detection across all three languages at the cost of a directory and about three small tests.
5. Versions: `tungstenite` 0.30 (daemon) versus 0.24 (controller) should be reconciled by whoever next touches the controller; not blocking.
6. Logging format: both use ad hoc `[terminal-agent]` and `[bridge]` println prefixes (the controller still says `[bridge]` at controller/src/main.rs:922, a leftover). A shared prefix convention helps journald filtering; trivial.

### 5.1 Spec-first and generated contracts (TypeSpec or similar): worth it at this size

Stated preference: spec-first and clean component separation. Honest evaluation for this repo, with moderate confidence (the JS-side tooling facts depend on what the frontend analysis finds):

- What a spec buys: one source of truth, drift detection, generated Rust structs and JS or d.ts types, generated docs replacing the prose in docs/terminal.md.
- What it costs here: the frontend is deliberately no-build, non-module `<script>` tags, ES6, vendored libraries (docs/terminal.md language section). Generated JS types are only valuable with a type checker; there is none in the loop (the CI job is a syntax check). So on the JS side generation yields documentation at best. On the Rust side, serde derive on tagged enums already is a precise, compiler-checked spec, and `controller/` already works this way with unit tests. A TypeSpec toolchain adds Node, an emitter pipeline, a build step and generated-file churn to a repo whose whole identity is "no build step" for the UI, to describe about 7 messages in the daemon (and perhaps 30 to 40 in the controller). As of my knowledge, Rust emitters for TypeSpec are less mature than the OpenAPI and JSON Schema ones (verify before relying on this).
- Cheaper middle path that keeps the spec-first spirit: write the contract as JSON Schema (hand-authored, or emitted once from TypeSpec and checked in), keep it in `docs/` or `protocol/schema/`, and use it as a test oracle rather than a code generator: both Rust tests and the node test validate the shared fixtures (point 4 above) against the schema, and the schema is the document of record that docs/terminal.md links to instead of duplicating message shapes. No codegen, no new build step in the shipped path, drift still caught in CI.
- Recommendation: do not adopt generated contracts for the daemon alone now. Adopt typed Rust enums plus shared golden fixtures immediately (cost: hours). Revisit generation if any of these becomes true: the controller and daemon protocols are merged into one crate (D), a second non-Rust, non-JS consumer appears (for example a host-side CLI or a test harness in Python), the combined message count passes roughly 40, or the frontend gains a build step or a type checker for other reasons. If the user wants spec-first regardless, the least regrettable form is TypeSpec emitting JSON Schema plus the fixture tests, with Rust types still hand-written against it, so that removing TypeSpec later loses nothing in the shipped code.

## 6. Proposed first slice, if approved

Smallest set that delivers value and leaves every option open, each as a separate green commit: (a) bind to loopback via a `listen` config key defaulting to `127.0.0.1` (smell 1; confirm no dev workflow depends on the wide bind first), (b) typed `protocol.rs` plus golden fixtures, (c) `clock/` isolated as a module, (d) real-PTY integration test for SIGWINCH and respawn, (e) generation counter, reaping and UTF-8 reassembly, (f) bounded per-client queues and targeted `session_list` and `clock_sync` delivery. Everything after that (scrollback replay, registry, actors, shared crate) is feature-driven.
