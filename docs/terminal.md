# On-Device Terminal

A third standalone deliverable, separate from the hardware-input bridge/UI
prototype ([`getting-started.md`](getting-started.md)) and the NOC
dashboard ([`dashboard.md`](dashboard.md)): a terminal emulator rendered by
the Chromium kiosk, backed by a real PTY on the device, reachable from a
Bluetooth-paired keyboard. Motivated by the feasibility research in
[`research/bt-hid-terminal-feasibility.md`](research/bt-hid-terminal-feasibility.md),
which this document implements the "implication for a terminal UI" section
of.

**Status: unverified scaffold.** No physical device was attached in the
session that produced this, or in the later session that ported the agent
from Python to Rust (see "Not yet verified on real hardware" below).
Everything here has been exercised on a desktop Linux host only.

## What it shows / does

A single-screen terminal with session tabs, backed by one or more PTY
processes (a local shell, or `ssh <host>` once a client is confirmed/added
per the research doc §4), typed into with a Bluetooth keyboard once
BlueZ's HID-host role is enabled, and navigated with the device's existing
5 physical buttons (session switch, interrupt, clear, respawn) and knob
(scrollback) exactly like the hardware-input prototype already does for
its own UI.

## Architecture

Two new pieces, plus zero changes to the existing `control-surface/agent/` daemon:

- **`terminal/frontend/`**: a static `index.html` + `style.css` + `app.js`
  page, no build step, no framework — the same pattern as `dashboard/frontend/`.
  It opens two local WebSockets:
  - `ws://localhost:8766` to `terminal/agent/` (Rust binary
    `terminal-agent`, this feature) for PTY input/output/resize/control.
  - `ws://localhost:8765` to the **existing, unmodified** `control-surface/agent/`
    daemon, read-only, to receive the existing raw `{"type":"button",...}`
    / `{"type":"knob",...}` messages for session-chrome (see "Bridge
    daemon: extension points" below for why this reads raw events
    instead of a new logical message type).
- **`terminal/agent/`** (Rust binary `terminal-agent`): on-device, spawns
  one PTY per entry in `terminal-agent-config.json` and relays I/O to the
  frontend over its own WebSocket. Config (shell vs `ssh user@host`) is
  data, not hardcoded, so it works identically whether or not an SSH
  client is present yet.

```
BT keyboard --(evdev, native Chromium input)--> xterm.js/placeholder (terminal/frontend)
                                                        |  ws://localhost:8766 (new)
                                                        v
                                         terminal/agent/ (Rust: terminal-agent)
                                                        |  libc::openpty() + std::process::Command
                                                        v
                                        local shell  /  ssh user@host (config-selected)

5 buttons + knob --(evdev)--> control-surface/agent/ (existing, UNCHANGED)
                                                        |  ws://localhost:8765 (existing)
                                                        v
                                   terminal/frontend's session-chrome handler
                                   (local button-code -> action table, client-side)
```

Why keyboard text input needs no `control-surface/agent/` or evdev changes at all: once a
paired BT keyboard surfaces as a normal `/dev/input/eventN` node (BlueZ
`input` plugin + kernel `uhid`, per the feasibility research), Chromium's
own native input stack turns keystrokes into ordinary `keydown`/`keyup`
DOM events automatically — exactly how the touchscreen already bypasses
the bridge daemon today (`research/hardware-interaction.md` point 2).
`terminal/frontend/app.js` just has a standard browser `keydown` handler.

## Language choices

- **`terminal/agent/`: Rust, matching `control-surface/agent/`'s own toolchain and build
  convention, superseding an earlier Python stdlib version.** This
  component originally shipped as a Python 3.7 stdlib script
  (`terminal-agent.py`, hand-rolling a minimal RFC 6455 WebSocket server
  because the stdlib has none) specifically to avoid paying for a second
  cross-compile target beyond `control-surface/agent/`'s own. It has since been rewritten
  in Rust and the Python source deleted; **no Python remains anywhere in
  this repo.** Reasoning for the rewrite:
  - `control-surface/agent/` already pays the `armv7-unknown-linux-gnueabihf` /
    `+crt-static` cross-compile cost once; paying it a second time for
    this component is no longer a reason to avoid Rust, and it removes
    the hand-rolled, materially less battle-tested WebSocket
    implementation the Python version carried as an explicit, acknowledged
    tradeoff.
  - **WebSocket server:** `tungstenite` 0.24, the exact same crate
    `control-surface/agent/src/main.rs` already depends on — specifically the sync
    variant (`tungstenite::accept` + one OS thread per client), not an
    async `tokio-tungstenite` runtime. `control-surface/agent/` itself is sync/threaded,
    not tokio-based, so this is the real in-repo precedent to match, not
    the async crate.
  - **PTY handling:** `libc::openpty` (a glibc/BSD extension present on
    the `gnueabihf` target) plus `std::process::Command` with
    `CommandExt::pre_exec` for `setsid()`, mirroring Python's
    `pty.openpty()` + `subprocess.Popen(..., preexec_fn=os.setsid)`
    one-to-one. `nix` was considered (it is the crate most commonly used
    for this in Rust, and does appear in `control-surface/agent/`'s own `Cargo.lock`) but
    checking `control-surface/agent/src/main.rs` directly shows it is only a *transitive*
    dependency of `evdev`, never called by `control-surface/agent/`'s own code — there is no
    actual direct-dependency precedent to match, so `libc` was used
    directly instead for a smaller, more predictable API surface.
  - Reading and writing the same sync `WebSocket` concurrently from
    separate threads (needed so PTY output broadcasts never block on, or
    get blocked by, a slow/idle client read) is done by `try_clone()`-ing
    the underlying `TcpStream` into a second, write-only `WebSocket`
    instance (`Role::Server`, no separate handshake needed since the TCP
    handshake response was already sent on the original clone target)
    rather than wrapping one `WebSocket` in a `Mutex` with a short read
    timeout. This was exercised directly (see "What was actually tested"
    below), not just reasoned about.
- **`terminal/frontend/`: static HTML/CSS/JS, non-module `<script>` tags,
  ES6-only** — forced, not chosen: the on-device Chromium 69 (2018)
  silently fails to parse ES2020 syntax (`?.`, `??`) with no console
  error, and a `file://` page load has no `Content-Type` header so a
  `type="module"` script tag never executes either. Both constraints are
  already documented in `docs/hardware.md`/`docs/getting-started.md` and
  re-derived nowhere here; `app.js` follows `dashboard/frontend/app.js`'s
  exact style for this reason.
- **xterm.js was not vendored.** This sandbox had no outbound network path
  to fetch the package. `terminal/frontend/app.js` ships a minimal
  `<pre>`-based line-buffer renderer that speaks the identical WebSocket
  JSON protocol a real xterm.js integration would use, so swapping it in
  later (see `terminal/frontend/vendor/README.md`) only touches the
  frontend's rendering functions, not the agent or the wire protocol.

## Wire protocol (frontend <-> agent, port 8766)

```
agent -> frontend:
  {"type": "session_list", "sessions": [{"index": 0, "name": "local-shell"}, ...]}
  {"type": "output", "session": 0, "data": "<decoded pty stdout chunk>"}
  {"type": "exited", "session": 0, "code": 0}

frontend -> agent:
  {"type": "input", "session": 0, "data": "<raw keystrokes>"}
  {"type": "resize", "session": 0, "cols": 80, "rows": 24}
  {"type": "control", "session": 0, "action": "interrupt" | "clear" | "respawn"}
```

Knob-scroll is NOT in this protocol at all: the placeholder renderer keeps
the full visible buffer in the DOM already, so scrolling is a pure
client-side DOM operation against `control-surface/agent/`'s existing `knob` messages,
no agent round-trip needed.

## Bridge daemon: extension point, deliberately NOT implemented

The task motivating this feature calls for session-chrome (knob
scroll/session-switch/interrupt/clear) as a new message-type extension to
`control-surface/agent/`'s existing local WebSocket protocol. **That extension was not
implemented in `control-surface/agent/src/main.rs`.** Instead, `terminal/frontend/app.js`
consumes `control-surface/agent/`'s existing, unmodified raw messages
(`{"type":"button","code":N,"value":1}` / `{"type":"knob","delta":N}`)
directly, and does the code-to-action translation client-side with a
placeholder table (`BUTTON_ACTIONS` in `app.js`).

Reasoning: `research/hardware-interaction.md`'s own "Open item carried
over" states the exact scancode-to-physical-button mapping is still
**unconfirmed**, pending a live `evtest`/`getevent -lt` capture on real
hardware. Teaching `control-surface/agent/` to translate raw codes into logical
`session_action`/`scroll` messages before that capture happens would mean
hardcoding a guessed mapping into the one process every other feature
(the existing button-test UI) also depends on, with a real risk of
breaking its currently-working behavior on a guess. Doing the translation
in this feature's own frontend file instead carries the same guessing
risk but confines it to a brand-new file nobody else depends on yet, and
touches zero bytes of `control-surface/agent/src`.

**Proposed schema for a future `control-surface/agent/` upgrade**, once the real mapping
is captured, so the frontend's `BUTTON_ACTIONS` table can be deleted and
replaced with direct consumption of these instead:

```
bridge -> any ws client (new, additive, alongside existing button/knob):
  {"type": "session_action", "action": "next_session" | "prev_session" | "interrupt" | "clear" | "respawn"}
  {"type": "scroll", "delta": N}
```

This would be purely additive to `control-surface/agent/src/main.rs` (a new
`BridgeEvent` variant plus a translation table keyed by the now-confirmed
`BUTTON_1`..`BUTTON_5` codes), landing after the open item above is
resolved, not before.

## Persistent deployment (future session, device attached)

Not run against real hardware in this session; written as instructions
for the next session that has the device attached, following the exact
read-only-rootfs-aware conventions already established in
[`getting-started.md`](getting-started.md) and [`dashboard.md`](dashboard.md).

This component needs no writable-at-runtime path at all (unlike the NOC
dashboard's metrics file): the agent keeps all PTY state in process
memory and writes nothing to disk, so everything it needs can live under
the read-only-after-boot `/opt` and `/usr/share` mounts, same as the
bridge daemon and its UI.

### 1. Push the frontend to its own on-demand path

```sh
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /usr/share/thing-revived-terminal"
adb push terminal/frontend/index.html terminal/frontend/style.css terminal/frontend/app.js \
  /usr/share/thing-revived-terminal/
adb shell "mount -o remount,ro /"
```

Like the bridge/UI prototype, this is NOT pushed to the default boot path
(`/usr/share/qt-superbird-app/webapp/`); it is reached on demand via the
same CDP `Page.navigate` trick documented in
[`getting-started.md`](getting-started.md#5-view-this-ui-on-demand-since-it-is-not-the-current-default),
targeting `file:///usr/share/thing-revived-terminal/index.html`.

### 2. Build, then push the agent binary, its config, and its supervisord entry

```sh
cd terminal/agent
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /opt/thing-revived"
adb push target/armv7-unknown-linux-gnueabihf/release/terminal-agent /opt/thing-revived/terminal-agent
adb shell "chmod +x /opt/thing-revived/terminal-agent"
adb push terminal-agent-config.json /opt/thing-revived/terminal-agent-config.json
adb shell "mount -o remount,ro /"
adb push ../deploy/supervisor.d/thing-revived-terminal.conf /etc/supervisor.d/thing-revived-terminal.conf
adb shell "supervisorctl reread && supervisorctl update"
```

Same `armv7-unknown-linux-gnueabihf` target and `+crt-static` static-linking
flag as `control-surface/agent/` (see [`getting-started.md`](getting-started.md#3-build-and-deploy-the-bridge-daemon-with-a-supervisord-entry-so-it-survives-reboot)
and [`hardware.md`](hardware.md)): static linking avoids a glibc-version
mismatch between the cross-toolchain and the device's older on-device
glibc.

Edit `terminal-agent-config.json` before pushing it to point the `ssh-devel`
entry at a real reachable host, or remove it if an SSH client isn't on the
device yet (see "SSH client" under Known limitations).

### 3. Verify

```sh
adb reboot
# wait for the device to reappear in `adb devices`
adb shell "supervisorctl status"
# expect thing-revived-terminal: RUNNING, alongside thing-revived-bridge
```

Then view it the same way as the bridge/UI prototype
(`getting-started.md`'s CDP `Page.navigate` steps), and confirm on-device
Chromium's keyboard events actually reach `app.js`'s `keydown` handler once
a BT keyboard is paired.

## Known limitations

- **Placeholder renderer, not real xterm.js.** No ANSI escape-sequence
  interpretation (cursor movement, color, alternate screen buffer for
  `vim`/`less`/etc. will look wrong). See `terminal/frontend/vendor/README.md`.
- **No `wss://`/TLS, no auth** on the `tungstenite` WebSocket server —
  acceptable for a loopback single-kiosk client, the same trust model
  `control-surface/agent/` already uses for its own WebSocket, but not suitable if this
  port were ever exposed beyond localhost. (The earlier Python version's
  hand-rolled RFC 6455 server, with its narrower text-frames-only,
  no-fragmentation limitation, no longer exists; this is now `control-surface/agent/`'s
  own proven `tungstenite` crate.)
- **No automatic wedge detection.** "Respawn if wedged" is a manual
  button-triggered action (`control`/`respawn`), not an automatic
  liveness check; nothing currently detects a hung PTY on its own.
- **Button-to-action mapping is a guess**, not a confirmed scancode
  table (see "Bridge daemon: extension point" above). Likely wrong until
  corrected by a live capture.
- **SSH client availability is unconfirmed** on yocto-superbird per
  `research/bt-hid-terminal-feasibility.md` §4 (sshd confirmed shipped,
  client not confirmed, expected trivially addable). The `ssh-devel`
  config entry will fail to spawn (`FileNotFoundError`/`ENOENT` from
  `subprocess.Popen`) until a client is confirmed or added.
- **`Ctrl-L`-as-clear** only works for programs that bind it to a screen
  clear (most shells do via readline); it is not a true terminal reset
  escape sequence.
- Only tested with one frontend client connected at a time; the agent
  broadcasts to all connected clients, but multi-client behavior (e.g. two
  browser tabs typing into the same session) was not exercised.

## Not yet verified on real hardware

No physical device was attached in the session that produced this
scaffold, nor in the later session that rewrote the agent from Python to
Rust. Everything below is unverified and must be checked once a unit is
available, following the pattern already used for every other
component's own verification section:

- The agent's own WebSocket server, PTY spawn/read/write/resize/interrupt,
  and the input→PTY→output round trip **were tested on a desktop Linux
  host** (not the device), both for the original Python version and again
  for the Rust rewrite, with a throwaway `/bin/sh -i` session and a
  minimal Python WebSocket test client: a real handshake, a
  `session_list` message, typed `echo HELLO_FROM_TEST` round-tripping
  back through PTY stdout, a `resize` message, and a `control`/`clear`
  message (which produced the expected `^L` byte back over the output
  channel) were all observed working on the Rust binary, same as the
  Python version before it. This confirms the protocol and the agent's
  core logic on a host x86_64 build, not on-device ARM behavior.
- The Rust rewrite's host-architecture (`x86_64-unknown-linux-gnu`) build
  was run with `cargo build`/`cargo build --release` and produced a clean
  compile. The `armv7-unknown-linux-gnueabihf` cross-target build (with
  `RUSTFLAGS="-C target-feature=+crt-static"`, matching `control-surface/agent/`) was
  also actually run in that same sandbox and produced a statically-linked
  ARM ELF binary (`file` confirmed: `ELF 32-bit LSB executable, ARM,
  EABI5, statically linked`) — the cross-compile step itself was
  exercised, not just assumed, but the resulting ARM binary was never
  copied to or run on the physical device.
- Whether a BT keyboard actually pairs and surfaces as
  `org.bluez.Input1` + a new evdev node on this exact BSP — the single
  open item flagged in `research/bt-hid-terminal-feasibility.md`'s
  "Overall verdict" — has not been checked at all.
- Whether Chromium 69 on-device actually delivers `keydown` events from
  that paired keyboard into this page's listener has not been checked.
- Whether an SSH client is actually present or can be added without a
  full image rebuild has not been checked.
- The persistent-deployment steps above (supervisord entry,
  remount-rw/ro sequencing, survives-reboot behavior) have not been run
  against the real unit; they follow the exact pattern already verified
  for `thing-revived-bridge` and `thing-revived-noc-agent` in
  `getting-started.md`/`dashboard.md`, but that is precedent, not a
  substitute for actually running it.
