# On-Device Terminal

A third standalone deliverable, separate from the hardware-input bridge/UI prototype ([`getting-started.md`](getting-started.md)) and the NOC dashboard ([`dashboard.md`](dashboard.md)): a terminal emulator rendered by the Chromium kiosk, backed by a real PTY on the device, reachable from a Bluetooth-paired keyboard. Motivated by the feasibility research in [`research/bt-hid-terminal-feasibility.md`](research/bt-hid-terminal-feasibility.md), which this document implements the "implication for a terminal UI" section of.

**Status: verified on real hardware (2026-11-15).** Deployed, exercised, and screenshotted against the physical unit — see "What was verified on real hardware (2026-11-15)" below, which also documents two real bugs found and fixed by that deployment. One blocker remains open on this unit's current firmware: real BT-keyboard pairing is still blocked pending a `yocto-superbird` reflash, see "Known limitations found on real hardware, still open" below.

## What it shows / does

A single-screen terminal with session tabs, backed by one or more PTY processes (a local shell, or `ssh <host>` once a client is confirmed/added per the research doc §4), typed into with a Bluetooth keyboard once BlueZ's HID-host role is enabled, and navigated with the device's existing 5 physical buttons (session switch, interrupt, clear, respawn) and knob (scrollback) exactly like the hardware-input prototype already does for its own UI.

## Architecture

Two new pieces, plus zero changes to the existing `prototype/control-surface/agent/` daemon:

- **`terminal/frontend/`**: a static `index.html` + `style.css` + `app.js` page, no build step, no framework — the same pattern as `prototype/dashboard/frontend/`. It opens two local WebSockets:
  - `ws://localhost:8766` to `terminal/agent/` (Rust binary `terminal-agent`, this feature) for PTY input/output/resize/control.
  - `ws://localhost:8765` to the **existing, unmodified** `prototype/control-surface/agent/` daemon, read-only, to receive the existing raw `{"type":"button",...}` / `{"type":"knob",...}` messages for session-chrome (see "Bridge daemon: extension points" below for why this reads raw events instead of a new logical message type).
- **`terminal/agent/`** (Rust binary `terminal-agent`): on-device, spawns one PTY per entry in `terminal-agent-config.json` and relays I/O to the frontend over its own WebSocket. Config (shell vs `ssh user@host`) is data, not hardcoded, so it works identically whether or not an SSH client is present yet.

```
BT keyboard --(evdev, native Chromium input)--> xterm.js (terminal/frontend)
                                                        |  ws://localhost:8766 (new)
                                                        v
                                         terminal/agent/ (Rust: terminal-agent)
                                                        |  libc::openpty() + std::process::Command
                                                        v
                                        local shell  /  ssh user@host (config-selected)

5 buttons + knob --(evdev)--> prototype/control-surface/agent/ (existing, UNCHANGED)
                                                        |  ws://localhost:8765 (existing)
                                                        v
                                   terminal/frontend's session-chrome handler
                                   (local button-code -> action table, client-side)
```

Why keyboard text input needs no `prototype/control-surface/agent/` or evdev changes at all: once a paired BT keyboard surfaces as a normal `/dev/input/eventN` node (BlueZ `input` plugin + kernel `uhid`, per the feasibility research), Chromium's own native input stack turns keystrokes into ordinary `keydown`/`keyup` DOM events automatically — exactly how the touchscreen already bypasses the bridge daemon today (`research/hardware-interaction.md` point 2). `terminal/frontend/app.js` just has a standard browser `keydown` handler.

## Language choices

- **`terminal/agent/`: Rust, matching `prototype/control-surface/agent/`'s own toolchain and build convention, superseding an earlier Python stdlib version.** This component originally shipped as a Python 3.7 stdlib script (`terminal-agent.py`, hand-rolling a minimal RFC 6455 WebSocket server because the stdlib has none) specifically to avoid paying for a second cross-compile target beyond `prototype/control-surface/agent/`'s own. It has since been rewritten in Rust and the Python source deleted; **no Python remains anywhere in this repo.** Reasoning for the rewrite:
  - `prototype/control-surface/agent/` already pays the `armv7-unknown-linux-gnueabihf` / `+crt-static` cross-compile cost once; paying it a second time for this component is no longer a reason to avoid Rust, and it removes the hand-rolled, materially less battle-tested WebSocket implementation the Python version carried as an explicit, acknowledged tradeoff.
  - **WebSocket server:** `tungstenite` 0.24, the exact same crate `prototype/control-surface/agent/src/main.rs` already depends on — specifically the sync variant (`tungstenite::accept` + one OS thread per client), not an async `tokio-tungstenite` runtime. `prototype/control-surface/agent/` itself is sync/threaded, not tokio-based, so this is the real in-repo precedent to match, not the async crate.
  - **PTY handling:** `libc::openpty` (a glibc/BSD extension present on the `gnueabihf` target) plus `std::process::Command` with `CommandExt::pre_exec` for `setsid()` followed by `ioctl(slave_fd, TIOCSCTTY)`, mirroring Python's `pty.openpty()` + `subprocess.Popen(..., preexec_fn=os.setsid)` plus one additional fix the Python version never needed to make explicit. `setsid()` alone makes the child a session leader with **no controlling terminal**; since the slave fd here is `dup()`'d rather than freshly `open()`'d, the kernel's normal "open() of an unowned tty by a ctty-less session leader" auto-attach path never fires either. Confirmed live: without the `TIOCSCTTY` ioctl, a `resize` WS message's `ioctl(master_fd, TIOCSWINSZ)` call produces zero visible effect — no `SIGWINCH` ever reaches the child, because the kernel only delivers it to a terminal's registered foreground process group, and without a controlling terminal there is none. This broke two things before the fix: `ssh` (session 0) never forwarded the real window size to the remote `glances` process (which rendered at its sshd-assigned fallback size instead of the kiosk's real width), and `bash` never redrew its prompt on resize. `nix` was considered (it is the crate most commonly used for this in Rust, and does appear in `prototype/control-surface/agent/`'s own `Cargo.lock`) but checking `prototype/control-surface/agent/src/main.rs` directly shows it is only a *transitive* dependency of `evdev`, never called by `prototype/control-surface/agent/`'s own code — there is no actual direct-dependency precedent to match, so `libc` was used directly instead for a smaller, more predictable API surface.
  - Reading and writing the same sync `WebSocket` concurrently from separate threads (needed so PTY output broadcasts never block on, or get blocked by, a slow/idle client read) is done by `try_clone()`-ing the underlying `TcpStream` into a second, write-only `WebSocket` instance (`Role::Server`, no separate handshake needed since the TCP handshake response was already sent on the original clone target) rather than wrapping one `WebSocket` in a `Mutex` with a short read timeout. This was exercised directly (see "What was actually tested" below), not just reasoned about.
- **`terminal/frontend/`: static HTML/CSS/JS, non-module `<script>` tags, ES6-only** — forced, not chosen: the on-device Chromium 69 (2018) silently fails to parse ES2020 syntax (`?.`, `??`) with no console error, and a `file://` page load has no `Content-Type` header so a `type="module"` script tag never executes either. Both constraints are already documented in `docs/hardware.md`/`docs/getting-started.md` and re-derived nowhere here; `app.js` follows `prototype/dashboard/frontend/app.js`'s exact style for this reason.
- **`terminal/frontend/`: real, vendored xterm.js 5.3.0 + FitAddon 0.8.0, not a placeholder.** `terminal/frontend/vendor/` ships the UMD `xterm.js` build, `xterm.css`, and `xterm-addon-fit.js` (see `terminal/frontend/vendor/README.md` for exact provenance). `app.js` creates one shared `Terminal` instance, writes every WebSocket `output` chunk straight to it via `term.write()`, and uses `FitAddon` to compute real `cols`/`rows` from the container's actual pixel size and the terminal's font metrics (see "Wire protocol" below). This gives full ANSI/VT100 and curses interpretation (cursor movement, color, alternate-screen apps like `glances`), not a raw line-buffer dump.

## Wire protocol (frontend <-> agent, port 8766)

```
agent -> frontend:
  {"type": "session_list", "sessions": [{"index": 0, "name": "local-shell"}, ...]}
  {"type": "output", "session": 0, "data": "<decoded pty stdout chunk>"}
  {"type": "exited", "session": 0, "code": 0,
   "auto_respawning": false, "needs_manual_respawn": false}
  {"type": "clock_sync", "offset_ms": 42, "status": "ok" | "stale" | "unsynced",
   "age_s": 12}

frontend -> agent:
  {"type": "input", "session": 0, "data": "<raw keystrokes>"}
  {"type": "resize", "session": 0, "cols": 118, "rows": 33}
  {"type": "control", "session": 0, "action": "interrupt" | "clear" | "respawn"}
```

`resize`'s `cols`/`rows` are computed live by `FitAddon` from `#screen`'s actual pixel size and the terminal's own font metrics on every connect/resize/tab-switch — the `118`/`33` above is just an example of what a real fitted value looks like, not a fixed constant; an earlier version of this agent/frontend pair sent a static `80`/`24` on every call, which is why `glances` never filled the kiosk's real screen width and (via `Session::resize()` only firing `SIGWINCH` when the new size actually differs from the previous one) a freshly-switched-to `bash` tab never redrew its prompt either.

`exited`'s `auto_respawning`/`needs_manual_respawn` fields and the standalone `clock_sync` message type are additive extensions beyond the original shape above; both are documented in full (including the retry/backoff and host-discovery logic behind them) in [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) §4 and §8, and implemented in `terminal/agent/src/main.rs`'s own doc comment at the top of that file.

Knob-scroll is NOT in this protocol at all: xterm.js owns scrollback internally (`term.scrollLines()`), so scrolling is a pure client-side operation against `prototype/control-surface/agent/`'s existing `knob` messages, no agent round-trip needed.

## Bridge daemon: extension point, deliberately NOT implemented

**Update (2026-11-16): this extension point was later implemented.** See [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) for the real mechanism actually shipped — exclusive `EVIOCGRAB` on `/dev/input/event0` plus selective `uinput` passthrough for `KEY_M`/`KEY_ENTER` inside `prototype/control-surface/agent/`, and a confirmed `BUTTON_SESSION_MAP` (real `KEY_*` codes, not placeholders) in `terminal/frontend/app.js` — which supersedes the speculative design described below. Kept for the historical record of the original design reasoning.

The task motivating this feature calls for session-chrome (knob scroll/session-switch/interrupt/clear) as a new message-type extension to `prototype/control-surface/agent/`'s existing local WebSocket protocol. **That extension was not implemented in `prototype/control-surface/agent/src/main.rs`.** Instead, `terminal/frontend/app.js` consumes `prototype/control-surface/agent/`'s existing, unmodified raw messages (`{"type":"button","code":N,"value":1}` / `{"type":"knob","delta":N}`) directly, and does the code-to-action translation client-side with a placeholder table (`BUTTON_ACTIONS` in `app.js`).

Reasoning: `research/hardware-interaction.md`'s own "Open item carried over" states the exact scancode-to-physical-button mapping is still **unconfirmed**, pending a live `evtest`/`getevent -lt` capture on real hardware. Teaching `prototype/control-surface/agent/` to translate raw codes into logical `session_action`/`scroll` messages before that capture happens would mean hardcoding a guessed mapping into the one process every other feature (the existing button-test UI) also depends on, with a real risk of breaking its currently-working behavior on a guess. Doing the translation in this feature's own frontend file instead carries the same guessing risk but confines it to a brand-new file nobody else depends on yet, and touches zero bytes of `prototype/control-surface/agent/src`.

**Proposed schema for a future `prototype/control-surface/agent/` upgrade**, once the real mapping is captured, so the frontend's `BUTTON_ACTIONS` table can be deleted and replaced with direct consumption of these instead:

```
bridge -> any ws client (new, additive, alongside existing button/knob):
  {"type": "session_action", "action": "next_session" | "prev_session" | "interrupt" | "clear" | "respawn"}
  {"type": "scroll", "delta": N}
```

This would be purely additive to `prototype/control-surface/agent/src/main.rs` (a new `BridgeEvent` variant plus a translation table keyed by the now-confirmed `BUTTON_1`..`BUTTON_5` codes), landing after the open item above is resolved, not before.

## Persistent deployment (future session, device attached)

Not run against real hardware in this session; written as instructions for the next session that has the device attached, following the exact read-only-rootfs-aware conventions already established in [`getting-started.md`](getting-started.md) and [`dashboard.md`](dashboard.md).

This component needs no writable-at-runtime path at all (unlike the NOC dashboard's metrics file): the agent keeps all PTY state in process memory and writes nothing to disk, so everything it needs can live under the read-only-after-boot `/opt` and `/usr/share` mounts, same as the bridge daemon and its UI.

### 1. Push the frontend to its own on-demand path

```sh
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /usr/share/thing-revived-terminal"
adb push terminal/frontend/index.html terminal/frontend/style.css terminal/frontend/app.js \
  /usr/share/thing-revived-terminal/
adb shell "mount -o remount,ro /"
```

Like the bridge/UI prototype, this is NOT pushed to the default boot path (`/usr/share/qt-superbird-app/webapp/`); it is reached on demand via the same CDP `Page.navigate` trick documented in [`getting-started.md`](getting-started.md#5-view-this-ui-on-demand-since-it-is-not-the-current-default), targeting `file:///usr/share/thing-revived-terminal/index.html`.

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

Same `armv7-unknown-linux-gnueabihf` target and `+crt-static` static-linking flag as `prototype/control-surface/agent/` (see [`getting-started.md`](getting-started.md#3-build-and-deploy-the-bridge-daemon-with-a-supervisord-entry-so-it-survives-reboot) and [`hardware.md`](hardware.md)): static linking avoids a glibc-version mismatch between the cross-toolchain and the device's older on-device glibc.

Edit `terminal-agent-config.json` before pushing it to point the `ssh-devel` entry at a real reachable host, or remove it if an SSH client isn't on the device yet (see "SSH client" under Known limitations).

### 3. Verify

```sh
adb reboot
# wait for the device to reappear in `adb devices`
adb shell "supervisorctl status"
# expect thing-revived-terminal: RUNNING, alongside thing-revived-bridge
```

Then view it the same way as the bridge/UI prototype (`getting-started.md`'s CDP `Page.navigate` steps), and confirm on-device Chromium's keyboard events actually reach `app.js`'s `keydown` handler once a BT keyboard is paired.

## Known limitations

- **No `wss://`/TLS, no auth** on the `tungstenite` WebSocket server — acceptable for a loopback single-kiosk client, the same trust model `prototype/control-surface/agent/` already uses for its own WebSocket, but not suitable if this port were ever exposed beyond localhost. (The earlier Python version's hand-rolled RFC 6455 server, with its narrower text-frames-only, no-fragmentation limitation, no longer exists; this is now `prototype/control-surface/agent/`'s own proven `tungstenite` crate.)
- **No automatic wedge detection.** "Respawn if wedged" is a manual button-triggered action (`control`/`respawn`), not an automatic liveness check; nothing currently detects a hung PTY on its own.
- **Button-to-action mapping** (see "Bridge daemon: extension point" above) — **resolved since this was written**, see the "Update (2026-11-16)" note there and [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md).
- **SSH client availability is unconfirmed** on yocto-superbird per `research/bt-hid-terminal-feasibility.md` §4 (sshd confirmed shipped, client not confirmed, expected trivially addable). The `ssh-devel` config entry will fail to spawn (`FileNotFoundError`/`ENOENT` from `subprocess.Popen`) until a client is confirmed or added.
- **`Ctrl-L`-as-clear** only works for programs that bind it to a screen clear (most shells do via readline); it is not a true terminal reset escape sequence.
- Only tested with one frontend client connected at a time; the agent broadcasts to all connected clients, but multi-client behavior (e.g. two browser tabs typing into the same session) was not exercised.

## What was verified on real hardware (2026-11-15)

Deployed and exercised for real against the physical unit, serial `8559R486QR1S`, still on stock DeskThing-modified firmware (not yocto-superbird, see "Known limitations" below for what that blocks).

- **Deployment**: pushed the `armv7-unknown-linux-gnueabihf` / `+crt-static` binary, `terminal-agent-config.json`, the frontend files, and the supervisord conf exactly per the steps above; `supervisorctl status` showed `thing-revived-terminal: RUNNING` alongside the pre-existing `thing-revived-bridge` and `thing-revived-noc-agent` units with no manual process management.
- **Two real bugs found and fixed by this deployment, not caught by the earlier desktop-only testing:**
  1. `Session::new` panicked the entire process (not just the one failing session) when a configured session's command couldn't spawn. On real hardware the shipped example config's `ssh-devel` entry fails exactly as `research/bt-hid-terminal-feasibility.md` predicted (no ssh client on this firmware), which crash-looped the whole agent under supervisord's `autorestart`. Fixed: `Session::new` now returns `Result` instead of panicking, `main()` logs and skips a failed session instead of aborting, and `dispatch()` now looks up sessions by their own `index` field instead of raw vector position (needed once a failed session can be absent from that vector).
  2. `control`/`respawn` correctly spawned a new child process and PTY, but the per-session output-reader thread exits permanently on its old child's EOF by design, and nothing restarted it for the new child — so a respawned session ran but went silently deaf, no output ever reached the frontend again. Fixed: `respawn`'s handler now spawns a fresh `pump_session_output` thread alongside the new child. Both fixed, rebuilt, redeployed, and re-verified in the same session (below); this file's earlier claim that host-only testing already covered the agent's "core logic" was true as far as it went, but real hardware surfaced real gaps host-only testing didn't.
- **Protocol round trip, for real, against the actual on-device shell**: a raw WebSocket test client (hand-rolled RFC 6455, same approach as the earlier host-only testing, now pointed at the device via `adb forward tcp:8766 tcp:8766`) got a real handshake, the `session_list` message, and `echo HELLO_FROM_REAL_DEVICE_TEST` typed over `input` round-tripping back through the PTY's actual `stdout` as an `output` message. Re-ran the same test immediately after a `control`/`respawn` message to confirm fix #2 above: output kept flowing on the new child, not just the original one.
- **Rendering, for real, on the actual on-device Chromium 69**: forwarded CDP port 2222, navigated the kiosk's live page to `file:///usr/share/thing-revived-terminal/index.html`, and screenshotted it: the `local-shell` session tab, the status bar's `AGENT: LIVE` / `BRIDGE: LIVE` indicators (confirming both the new terminal WebSocket and the pre-existing, unmodified bridge daemon's WebSocket connect successfully from this page), and a correct render of typed input and real shell output (`echo CLEAN_FINAL_TEST` → `CLEAN_FINAL_TEST` → a fresh `#` prompt) were all confirmed visually.
- **Keyboard input path, verified by proxy, not by a real BT keyboard**: no BT keyboard was paired this session (blocked by the firmware gap below). Instead, CDP's `Input.dispatchKeyEvent` was used to inject real `keydown`/`keyup` DOM events at the browser level after focusing `#screen`, the same event type a real paired keyboard would generate per the design in "Architecture" above. This proves the page's `keydown` handler → WebSocket `input` message → PTY write path works end to end on real Chromium 69, but it is a proxy for, not a replacement for, confirming an actual BT keyboard surfaces as an evdev node on this hardware, which still needs the test described below.
- Device was left in its normal state afterward: kiosk navigated back to the default dashboard page, `adb forward --remove-all` run, no persistent change to which app is the default boot screen.

## Known limitations found on real hardware, still open

**Update (2026-11-16): most items below were resolved since this section was written.** See [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) for the real EVIOCGRAB+uinput button mapping, the clock-sync design, and the bounded auto-respawn policy, and `terminal/agent/src/main.rs` / `prototype/control-surface/agent/src/main.rs` for the shipped implementation of all of it. The device-firmware/BT-pairing item is the one still genuinely open; kept below for the historical record of exactly what this 2026-11-15 hardware session found.

- **This unit is still on stock DeskThing-modified firmware, not yocto-superbird.** Per `research/bt-hid-terminal-feasibility.md`, stock firmware's `bluetoothd` explicitly whitelists `--plugin=gap,deviceinfo`, excluding the `input` (HID-host) plugin, so a real BT keyboard cannot be paired as an input peripheral on this firmware as currently flashed. Confirmed again this session: `hciconfig -a hci0` shows the radio up (`UP RUNNING AUTH ENCRYPT`), consistent with prior findings, with no change to the plugin restriction. **Reflashing to yocto-superbird is a separate, bigger, more-irreversible step than this deployment and was deliberately not done in this session without explicit sign-off.**
- SSH client presence on this firmware: confirmed absent (`ssh-devel` session's `ENOENT` on spawn), consistent with `bash-availability.md`/`bt-hid-terminal-feasibility.md`'s prediction. The agent now degrades gracefully instead of crashing, per fix #1 above, but an actual SSH session still needs either a client installed on this firmware or the yocto-superbird reflash.
- Real button/knob scancode-to-action mapping: resolved since, see the update note at the top of this section.
