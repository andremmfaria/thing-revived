# Design: buttons 1-4 direct session switch, knob scroll, clock widget

Status: design only, no code written. Companion implementation ticket should reference this file section-by-section.

## 0. Premise correction (read this first)

The task brief that generated this document assumed the fix for buttons/knob belongs in `terminal/agent` (the PTY-relay daemon, port 8766, `main.rs` reviewed in full). **That assumption is wrong and the design below does not follow it.** Evidence from the actual repo:

- `prototype/control-surface/agent` (crate name `superbird-bridge`, binary listens on `ws://0.0.0.0:8765`) already opens `/dev/input/event0` (gpio-keys-polled: buttons 1-4, M, knob-push/KEY_ENTER) and `/dev/input/event1` (rotary, `REL_HWHEEL`) directly via the `evdev = "0.12"` crate, and already broadcasts every `EV_KEY` event on event0 as `{"type":"button","code":N, "value":V}` and every `EV_REL` event on event1 as `{"type":"knob", "delta":D}`. This is the process with evdev access, not `terminal/agent`.
- `terminal/frontend/app.js` already has a second WebSocket client (`connectBridge`, `BRIDGE_WS_URL = ws://localhost:8765`) consuming exactly those two message types, plus `scrollActiveScreen(delta)` wired to `'knob'` messages, plus `handleButtonAction()` wired to `'button'` messages via a `BUTTON_ACTIONS` lookup table.
- `terminal/agent` (`main.rs`, port 8766) only ever sees `input` / `resize` / `control` messages from the frontend. It has no evdev access, no reason to gain any, and the task's instruction to "evaluate EVIOCGRAB ... in the terminal-agent Rust process" is therefore based on a wrong mental model of which binary owns the hardware.

**Consequence for requirement 3 (knob scroll):** it is already implemented. `app.js`'s own top-of-file comment ("app.js currently only listens for keydown, not wheel/rel events, so rotation currently does nothing") describes a *different, hypothetical* approach — making the browser itself synthesize native DOM `wheel` events from `REL_HWHEEL` — which was never built and isn't needed, because the bridge's `'knob'` WS message already carries the rotation delta straight into `scrollActiveScreen()`. No kernel-level `wheel` event synthesis, no grab of `event1`, nothing to do here beyond verifying it live and tuning `KNOB_SCROLL_LINES_PER_STEP`. Section 3 below is a verification step, not a build step.

**Consequence for requirement 1 (buttons 1-4):** the actual bug is a double delivery path, not missing code:
1. `prototype/control-surface/agent` already reads and broadcasts raw codes for *all* keys on event0 (1-4, M, knob-push), but `app.js`'s `BUTTON_ACTIONS` table is keyed by placeholder values `1..5` (per its own comment: "Raw evdev KEY_* codes are UNCONFIRMED ... placeholders"), not the real Linux keycodes (`KEY_1`=2, `KEY_2`=3, `KEY_3`=4, `KEY_4`=5, `KEY_M`=50, `KEY_ENTER`=28). So today, `BUTTON_ACTIONS[msg.code]` never matches anything for a real press — this path is currently dead code.
2. Because `gpio-keys-polled` registers as an ordinary Linux keyboard input device, Chromium's own input stack *also* turns every press into a plain DOM `keydown` on whatever element has focus — which is `#screen` (see `main()`'s `.focus()` call). `onKeyDown()`'s generic `evt.key.length === 1` branch then relays `'1'`..`'4'`, `'m'` as literal PTY input, and `KEY_SEQUENCES.Enter` handles Enter. **This is the only path that currently does anything**, and it is exactly why buttons 1-4 type literal digits today, and why M/Enter already work as plain keystrokes without any bridge-side mapping at all.

So the fix is: suppress the OS-keydown path for 1-4 only (requirement 1), keep it alive for M/Enter (requirement 2, for free, zero app.js change needed for those two keys), and fix the dead `BUTTON_ACTIONS` table so the bridge path becomes the live path for 1-4 only. All of this lands in `prototype/control-surface/agent` + `terminal/frontend/app.js`. `terminal/agent` (`main.rs`) gets **no changes** for requirements 1 and 3.

---

## 1. Requirement 1+2: exclusive grab + selective uinput re-injection

### Decision

In `prototype/control-surface/agent`, after opening `/dev/input/event0`:

1. Call `Device::grab()` (`evdev` 0.12 wraps `EVIOCGRAB`) immediately after open, before the read loop starts. This makes the kernel deliver event0's events to `prototype/control-surface/agent`'s fd **exclusively** — no other reader (X11/Wayland/libinput/Chromium's raw-input backend, whatever turns this device into DOM keydowns) sees any of it anymore. All 6 physical keys (1-4, M, knob-push/Enter) stop reaching the OS-keydown path the instant this grab succeeds.
2. Build one `uinput` virtual keyboard device at startup (`evdev::uinput::VirtualDeviceBuilder`, enabling exactly `KEY_M` (50) and `KEY_ENTER` (28) in its key-capability set — nothing else).
3. In the existing button read loop, for every `EV_KEY` event read from the grabbed real device:
   - Always broadcast it over the existing WS `'button'` message, unchanged (`{"type":"button","code":N,"value":V}`) — grabbing a device only affects *other* listeners, not the fd `prototype/control-surface/agent` itself already holds open, so this requires no change to the broadcast call.
   - If `code == KEY_M (50)` or `code == KEY_ENTER (28)`, additionally replay it (same `value`: press/release/repeat) through the uinput device, so Chromium's input stack sees a normal keyboard event for those two keys only, from a device that was never grabbed.
   - If `code` is one of `KEY_1..KEY_4` (2,3,4,5), do **not** replay it anywhere else — it only exists as the WS `'button'` message now.

This is additive to the existing `run_buttons()` function; `run_knob()` (`/dev/input/event1`) is untouched — no grab, no uinput, per the premise correction above.

### Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Client-side filtering in `app.js` (ignore digit keydowns somehow) | Explicitly ruled out by the confirmed hardware fact: Chromium cannot distinguish a gpio button's synthesized keydown from a real/future BT keyboard's digit key — there is no source-device tag left by the time JS sees the event. Any JS-side heuristic (e.g. "ignore bare digit keys") would also break a real keyboard typing digits into the terminal, which is a real, intended use case for this device. |
| udev hwdb keycode remap | Ruled out already in the task's own hardware findings: `gpio-keys-polled` assigns `linux,code` straight from devicetree with no separate raw-scancode layer for hwdb to intercept. |
| Devicetree/kernel remap (e.g. to `KEY_F13..F16`) | The architecturally cleanest long-term fix, but requires an image/kernel rebuild, explicitly out of scope per the task brief. Worth flagging as a future replacement for the whole grab/uinput mechanism if/when a rebuild pipeline exists — at that point this entire section becomes unnecessary and can be deleted. |
| Grab + re-inject M/Enter via Chrome DevTools Protocol (`Input.dispatchKeyEvent`) instead of uinput | Works, but couples `prototype/control-surface/agent` to Chromium's remote-debugging port (extra TCP connection, CDP session lifecycle, version-specific protocol quirks on Chromium 69) for no benefit over uinput, which is a few kernel ioctls, transport-agnostic, and survives if the kiosk browser is ever swapped. Rejected on complexity grounds, not correctness. |
| Grab event0 and re-inject *all six* codes, doing the 1-4 remap at the uinput layer instead of client-side (e.g. uinput replays `KEY_F13` for physical "1") | Would also work and keeps `app.js`'s existing keydown-based dispatch table idea alive for buttons too. Rejected because: (a) it reintroduces exactly the ambiguity problem above one level removed — the frontend would need a dedicated keydown handler for F13-16 that must never fire from a real keyboard with F-keys, fragile; (b) the WS `'button'` message channel already exists, is already correctly delivering code 1-4 (it's just unconsumed today), and is the strictly simpler path requiring zero kernel-event synthesis for those four keys. |

### Failure mode

If `prototype/control-surface/agent` crashes or is stopped, the kernel releases the grab automatically when its fd closes, and buttons 1-4 revert to leaking digits into the OS-keydown path again until the daemon restarts (degraded, not broken — identical to today's behavior). Mitigate with `Restart=always` + a short `RestartSec` in `prototype/control-surface`'s systemd unit (confirm this is already set; if not, add it as part of this change).

### Exact codes to use (from live-confirmed hardware; verify once with `evtest` or the bridge's own `println!` before merging)

```
KEY_1     = 2
KEY_2     = 3
KEY_3     = 4
KEY_4     = 5
KEY_M     = 50
KEY_ENTER = 28
```

### `app.js` changes

No new WS message type — reuse the existing `'button'` shape, fix the consumer:

```js
// Replace the placeholder BUTTON_ACTIONS table. Keys are now the REAL
// Linux KEY_* codes (confirmed live), not 1..5 placeholders. Action is a
// direct session index, not next/prev — requirement 1.
const BUTTON_SESSION_MAP = {
  2: 0,  // KEY_1 -> session 0
  3: 1,  // KEY_2 -> session 1
  4: 2,  // KEY_3 -> session 2
  5: 3,  // KEY_4 -> session 3
};
```

```js
// In connectBridge()'s onmessage, 'button' branch:
if (msg.type === 'knob') {
  scrollActiveScreen(msg.delta);
} else if (msg.type === 'button' && msg.value === 1) {
  if (Object.prototype.hasOwnProperty.call(BUTTON_SESSION_MAP, msg.code)) {
    switchToSession(BUTTON_SESSION_MAP[msg.code]);
  }
  // KEY_M (50) / KEY_ENTER (28) intentionally absent from the map: their
  // behavior is unchanged and arrives through the normal keydown path
  // (uinput-replayed by prototype/control-surface/agent), exactly as before.
}
```

```js
// New function, replaces the old next_session/prev_session branch of
// handleButtonAction (interrupt/clear/respawn's current button triggers
// go away since there are only 4 numbered physical buttons + M + knob-push
// on this hardware, not 5 — confirmed live. If interrupt/clear/respawn need
// a trigger, that's a separate decision for the user, out of scope here;
// they remain reachable via 'control' messages if some other UI is added).
function switchToSession(index) {
  const exists = state.sessions.some(function (s) { return s.index === index; });
  if (!exists) return; // no such session configured; no-op, not a crash
  if (index === state.activeSession) return;
  state.activeSession = index;
  renderTabs();
  renderScreen();
  sendAgent({ type: 'resize', session: state.activeSession, cols: 80, rows: 24 });
}
```

Remove `BUTTON_ACTIONS`, `handleButtonAction()`'s `next_session`/`prev_session` branch, and the `5: 'respawn'` placeholder entry (dead per the 4-button+M+knob hardware reality). Leave `interrupt`/`clear`/`respawn` control-message plumbing in `terminal/agent` untouched (`main.rs`'s `dispatch()` already handles `"control"` messages generically) — only this button's *trigger* is being removed, not the underlying capability.

---

## 2. `prototype/control-surface/agent` Cargo/code delta summary (file-by-file)

- `prototype/control-surface/agent/Cargo.toml`: no new dependency — `evdev = "0.12"` already provides both `Device::grab()`/`ungrab()` and `evdev::uinput::VirtualDeviceBuilder`.
- `prototype/control-surface/agent/src/main.rs`:
  - `open_device()`: after `Device::open(path)` succeeds for `event0` specifically, call `.grab()`; log success/failure (failure should be loud — it means buttons 1-4 are back to leaking digits).
  - New function `build_uinput_passthrough() -> uinput::VirtualDevice`, called once in `main()` before spawning `run_buttons`, capability set = `{KEY_M, KEY_ENTER}` only. Pass the resulting handle into `run_buttons` (needs a signature change: `run_buttons(path, clients, uinput_dev)`).
  - `run_buttons()`: inside the existing `if ev.event_type() == EventType::KEY` branch, after the existing `broadcast(...)` call, add the `code == 50 || code == 28` check and call `uinput_dev.emit(&[...])` with the same `InputEvent` (press/release/repeat value) plus a `SYN_REPORT`, mirroring the kernel event framing uinput expects.
  - `run_knob()`: **unchanged**.
  - `main()`: wire the new uinput device into `run_buttons`'s spawn call.

No WS protocol change in `prototype/control-surface/agent` — `'button'` and `'knob'` message shapes are unchanged.

---

## 3. Requirement 3 verification (not a build task)

1. Confirm `prototype/control-surface/agent`'s `run_knob()` is live and connected (status dot `BRIDGE: LIVE` in the frontend status bar already implies this).
2. Rotate the knob while a terminal session has enough scrollback to move, confirm `scrollActiveScreen()` fires (console.log or visually). If it already scrolls, this requirement is done; only tune `KNOB_SCROLL_LINES_PER_STEP` (currently `2`) and the `lineHeight` approx (`17`, hardcoded, matches `.screen`'s CSS — reconfirm against `style.css` if the font changes) to taste.
3. If (and only if) live testing shows the rotary device *also* generates a native DOM `wheel` event through some other OS-level path (unlikely — `REL_HWHEEL` alone, with no accompanying `BTN_*`, is not normally promoted to a pointer/mouse device by generic input stacks, and nothing in `app.js` currently has a `wheel` listener to double-count against), only then consider grabbing `event1` too. Do not pre-emptively grab it; there is no evidence of a problem to fix there.

---

## 4. Requirement 4: clock widget synced against vermelho

### Decision: periodic SSH `date` probe from `terminal/agent`, offset-based, broadcast over the existing WS 8766 connection

Lives in `terminal/agent` (not `prototype/control-surface/agent`): it already owns the one WS connection every frontend tab already holds open for terminal I/O, it's already root, and the task explicitly earmarks the existing `/var/lib/thing-revived/.ssh/id_ed25519` trust for "this kind of cross-host command," which matches a short-lived `ssh ... date` exec far better than opening a second purpose-built channel.

#### Host discovery (vermelho's IP, without hardcoding it)

Precedence, highest first:
1. `TERMINAL_AGENT_VERMELHO_HOST` env var, or a new optional `"vermelho_host"` key in `terminal-agent-config.json` (same file already loaded by `load_config()`) — explicit provisioning-time override, for when the link topology changes (e.g. a future non-point-to-point network).
2. Otherwise, parse the device's own default route: run `ip route show default` (or read `/proc/net/route` directly to avoid a subprocess — either is fine, `ip route` is simpler to get right first) and extract the `via` address. On this point-to-point USB-CDC-NCM link, vermelho *is* the gateway dhcpcd leased, so this tracks the current IP automatically across reconnects without ever hardcoding `10.42.1.30`.
3. Re-resolve this on every sync cycle (cheap, a few ms), not once at startup — the whole point is surviving a lease change without a terminal-agent restart.
4. **Shipped 4th fallback, confirmed against `terminal/agent/src/main.rs`'s own `discover_vermelho_host()`/`subnet_probe_fallback()`:** this link's actual live topology turned out to have **no default route at all** — just a directly-connected `/29` scope-link subnet with no `via` gateway, so step 2 above returns nothing to parse in practice. Rather than hardcode vermelho's address (explicitly disallowed), the shipped fallback parses `ip -4 -o addr show` for the device's own primary non-loopback IPv4 address + prefix, derives the subnet (bounded to `/28` or smaller, i.e. at most 16 hosts, to avoid turning into an accidental port scanner on a larger network), and TCP-probes port 22 on every other host in that small range with a 300ms timeout, returning the first that answers. This re-resolves on every cycle exactly like the default-route path would, just via a different signal given this topology's real constraints — it is the fallback that actually fires on the deployed link, not step 2.

#### Sync mechanism

Every 60s (configurable constant, e.g. `CLOCK_SYNC_INTERVAL_S`), on a dedicated thread:

```
t0 = Instant::now() + SystemTime::now()   // local wall-clock + monotonic anchor
run: ssh -i /var/lib/thing-revived/.ssh/id_ed25519 \
         -o BatchMode=yes -o ConnectTimeout=3 -o StrictHostKeyChecking=accept-new \
         andremmfaria@<discovered-host> 'date +%s%3N'
t1 = Instant::now() + SystemTime::now()
```

- `BatchMode=yes`: never hang on a password/passphrase prompt if the key ever stops being trusted — fail fast instead.
- `ConnectTimeout=3`: bound worst-case stall; this runs on its own thread so it never blocks PTY I/O either way, but a tight bound keeps the sync cadence meaningful.
- `StrictHostKeyChecking=accept-new`: avoids a one-time interactive host-key prompt after a vermelho reinstall, without fully disabling host-key verification.
- Round-trip `rtt = t1 - t0` (wall-clock duration of the whole exec). Estimate remote time is `rtt/2` ahead of what `date` reported by the time the answer is parsed back (classic NTP-style symmetric-latency assumption — adequate for a point-to-point USB link with sub-10ms RTT; not claiming NTP-grade accuracy, just "close enough for a wall clock widget", explicitly scoped as such). `remote_estimate_ms = parsed_epoch_ms + rtt_ms / 2` `offset_ms = remote_estimate_ms - local_epoch_ms_at_t1`
- Store `offset_ms` and `last_sync_at` (monotonic `Instant`) in shared state (`Arc<Mutex<ClockState>>`, same pattern as `Clients`).
- Broadcast `{"type":"clock_sync","offset_ms":<i64>,"status":"ok"}` to all connected frontend clients (same `broadcast()` helper already used for `output`/`exited`/`session_list`).
- Also send the current cached `clock_sync` state immediately to a newly connected client, right after `session_list_message()` in `handle_client()`, so a freshly opened tab doesn't show "unsynced" for a full sync interval.

#### Failure handling (vermelho briefly unreachable)

- `ssh` non-zero exit, timeout, or unparseable output: do **not** zero out `offset_ms` — keep the last good offset (device clock drift over a 60s-to-few-minutes gap is negligible compared to the value of not flickering the display). Broadcast `{"type":"clock_sync","offset_ms":<last known>,"status":"stale", "age_s":<seconds since last_sync_at>}` so the frontend can show a visual "unconfirmed" indicator (e.g. the clock text in amber instead of green) without losing the clock itself.
- Backoff on repeated failure: retry at 10s, 20s, 40s, capped at the normal 60s interval, instead of hammering SSH against an unreachable host; reset to the normal 60s cadence on the next success.
- If the daemon has never synced successfully since its own process start (no cached offset at all), broadcast `{"type":"clock_sync","offset_ms":0,"status":"unsynced"}` so the frontend shows the device's raw local clock (better than a blank widget) with the unconfirmed indicator from the very first connection.

#### `terminal/agent/src/main.rs` changes (file-by-file)

- New module-level constants: `CLOCK_SYNC_INTERVAL_S`, SSH key path, ssh binary args.
- New struct `ClockState { offset_ms: i64, status: ClockStatus, last_sync: Option<Instant> }` behind `Arc<Mutex<_>>`, constructed in `main()` alongside `clients`.
- New function `discover_vermelho_host() -> Option<String>` (env/config override, else parse `ip route show default`).
- New function `clock_sync_loop(clients: Clients, clock: Arc<Mutex<ClockState>>)` spawned once in `main()` via `thread::spawn`, doing the probe/backoff/broadcast logic above.
- `handle_client()`: after the existing `broadcast(&clients, &session_list_message(&sessions));` line, also send the current `clock` snapshot to the newly joined client the same way.
- `ConfigFile`/`load_config()`: add an optional `vermelho_host: Option<String>` field to the top-level config struct (sessions array stays as-is), read once at startup, passed into `discover_vermelho_host()` as the top override.
- No change to the `input`/`resize`/`control`/`output`/`exited`/`session_list` message shapes — `clock_sync` is a new, independent, additive message type on the same connection.

#### `terminal/frontend` changes (file-by-file)

- `terminal/frontend/index.html`: add a clock widget element, bottom-right, alongside the existing `.status-bar` convention (new small fixed-position block, not inside `.status-bar` itself since that's a full-width footer row already carrying agent/bridge/session status — the clock is explicitly requested bottom-right of the *screen*, i.e. an overlay on `.screen-wrap`):

  ```html
  <div class="screen-wrap">
    <pre class="screen" id="screen" tabindex="0"></pre>
    <div class="clock-widget" id="clock-widget">
      <span class="status-dot" id="clock-dot"></span>
      <span id="clock-text">--:--:--</span>
    </div>
  </div>
  ```

- `terminal/frontend/style.css`: add `.clock-widget { position: absolute; bottom: <n>px; right: <n>px; ... }` (parent `.screen-wrap` needs `position: relative` if it doesn't have it already — check before assuming); reuse `.status-dot`/`.status-dot.ok` classes already defined for the ok/not-ok coloring convention, add a `.status-dot.stale` variant (amber) for the "last known, unconfirmed" state.
- `terminal/frontend/app.js`:
  - `state`: add `clockOffsetMs: 0`, `clockStatus: 'unsynced'`.
  - `connectAgent()`'s `onmessage`: add
    ```js
    } else if (msg.type === 'clock_sync') {
      state.clockOffsetMs = msg.offset_ms;
      state.clockStatus = msg.status;
      renderClock();
    }
    ```
  - New `renderClock()`: formats `new Date(Date.now() + state.clockOffsetMs)` into `#clock-text`, sets `#clock-dot`'s class (`ok` -> green, `stale`/`unsynced` -> amber/absent) matching the existing `setStatus()` pattern (can reuse `setStatus('clock-dot', 'clock-text', ...)` almost as-is, just feeding it a formatted time string instead of a fixed label).
  - `main()`: add `setInterval(renderClock, 1000);` so the displayed time keeps ticking every second between sync broadcasts — the sync message only needs to arrive once a minute; the 1Hz tick is purely local arithmetic on `Date.now() + clockOffsetMs`, no new network traffic.

---

## 5. Summary of exact new WS payloads

Only one new message type in the whole design, on the existing `terminal/agent` (8766) connection; `prototype/control-surface/agent` (8765)'s `'button'`/`'knob'` shapes are unchanged:

```
agent -> frontend (new):
  {"type":"clock_sync","offset_ms":<i64 ms, local-clock-to-vermelho offset>,
   "status":"ok"|"stale"|"unsynced","age_s":<u64, present when status != "ok">}
```

No new `frontend -> agent` message types at all — session switching and scrolling are resolved entirely client-side in `app.js` from the bridge's pre-existing `'button'`/`'knob'` messages.

---

## 6. Open items deliberately left out of this design (flag to the user, don't silently decide)

- `interrupt`/`clear`/`respawn` currently had button triggers (`BUTTON_ACTIONS` codes 3, 4, 5) that are being removed because the hardware only has 4 numbered buttons + M + knob-push, not 5, and all 4 numbered buttons are now claimed by direct session switching per requirement 1. Partially resolved below (§8): manual respawn now has a trigger again (re-selecting a dead session), but `interrupt`/`clear` still have no physical trigger. If they need one, that's a separate decision — not decided here.
- `KEY_M`'s current behavior is "types the literal character 'm'" per the task's own requirement 2 wording. If M is ever wanted as a modifier key for the above, this design's "keep M's behavior unchanged" directly conflicts with that — flagging now so it isn't rediscovered later as a surprise.

---

## 7. Requirement 5: KEY_ESC as a hard "unwedge" control

Full key map on `/dev/input/event0`, now confirmed: `KEY_ESC=1`, `KEY_1..KEY_4=2..5`, `KEY_ENTER=28`, `KEY_M=50`. The 7th key (ESC, below the knob) is designated the hard-recovery button: kill and restart the entire `terminal-agent` process (all sessions torn down and respawned fresh), equivalent to `systemctl restart <terminal-agent unit>`.

### Decision: handle it entirely inside `prototype/control-surface/agent`, not via a WebSocket round-trip to `terminal/agent` or through `app.js`

Rationale: this is explicitly a *recovery* control for when something is stuck. A design that routes "please restart terminal-agent" through `app.js`'s JS event loop and a live WebSocket to `terminal/agent` is exactly the kind of path that can itself be wedged (frozen tab, crashed renderer, `terminal/agent` already hung and not reading its socket) at the moment the user most needs this button to work. `prototype/control-surface/agent` is a separate process, already root, already has the grabbed fd for this exact key, and has no dependency on Chromium or on `terminal/agent`'s health. Triggering the restart directly from there removes every one of those failure modes from the critical path.

- `KEY_ESC` (code 1) is already swept into the `EVIOCGRAB` on `/dev/input/event0` from §1 — nothing extra needed to stop it leaking to the browser as a literal Escape keydown; it simply is not in the uinput-replay allowlist (`{KEY_M, KEY_ENTER}`), same as codes 2-5.
- `run_buttons()`'s existing per-event handling gets one more branch: on `code == 1 && value == 1` (press, not release/repeat), call `Command::new("systemctl").args(["restart", TERMINAL_AGENT_UNIT]).spawn()` (fire-and-forget; don't block the button-read loop waiting on systemctl). `TERMINAL_AGENT_UNIT` is a new constant — confirm the actual unit name on the device (used as a placeholder `thing-revived-terminal-agent.service` above; verify against the real installed unit file before wiring this in) and requires `terminal-agent`'s unit to have `Restart=always` or at minimum be a plain `systemctl restart`-able service (it already is, by definition, if it's a systemd unit — just confirm it isn't `Type=oneshot` or similarly unsuitable).
- Still also broadcast the normal `{"type":"button","code":1,"value":V}` message over the existing WS 8765 channel, unchanged — harmless, and `app.js` already ignores unmapped codes, so no frontend change is strictly required for this key. (Optional, not required: `app.js` could show a transient "RESTARTING TERMINAL AGENT…" banner on seeing code 1 — cosmetic, skip unless the user wants it later, since `terminal/agent`'s own WS disconnect + `connectAgent()`'s existing 2s reconnect loop already gives a visible "AGENT: DISCONNECTED" → "AGENT: LIVE" cycle for free.)

### Why not restart from inside `terminal/agent` itself (self-`exit`)

Considered: `terminal/agent` receiving a new `{"type":"agent_control", "action":"restart"}` WS message from `app.js` and calling `std::process::exit()` so systemd restarts it. Rejected as the primary mechanism for the reason above (depends on the exact process chain that may be the thing that's stuck), but it's cheap to *also* add as a secondary trigger reachable from the UI for the non-emergency case ("I'd like a clean restart" without reaching for the physical button) — optional, not required by this requirement, not designed further here to avoid scope creep.

---

## 8. Requirement 6: detect and handle a session's child exiting after a successful spawn

### What's already there vs. the actual gap

`terminal/agent`'s `pump_session_output()` (`main.rs`) already detects this at the OS level: when a child's PTY slave closes (clean exit: `read()` returns `0`; or `EIO`, the common "slave closed" signal), it already broadcasts `{"type":"exited","session":N,"code":Option<i32>}` and stops that session's reader thread. So detection itself is not actually missing — the real gap, confirmed by the live `ssh-devel` (session 1, pid 909) incident, is **no recovery path reaches the user**: the frontend historically pointed at a `BUTTON_ACTIONS[5] = 'respawn'` trigger, but (a) hardware has no 5th button, and (b) that code was dead anyway per §0's placeholder-code bug, and after §1's cleanup button 5 doesn't exist as a concept at all. So today a dead session is, in practice, unrecoverable without an SSH-in-from-vermelho or a full service restart (the ESC button from §7, but that's a blunt instrument that kills *every* session, not just the one that died).

### Decision: bounded automatic respawn server-side, plus a zero-new-button manual trigger client-side, reusing the existing `exited` and `control`/`respawn` message shapes

**Server side (`terminal/agent`):**

- Add a per-`Session` `respawn_attempts: Mutex<Vec<Instant>>` (or a small ring buffer) tracking recent *involuntary* exits (i.e. exits detected by `pump_session_output`'s EOF/EIO path, not ones initiated by an explicit `"control"/"respawn"` message — keep these counted separately so a user manually respawning repeatedly never trips the auto-backoff limiter meant for crash-looping commands).
- On involuntary exit, instead of just broadcasting `exited` and stopping:
  1. Record the exit instant.
  2. If fewer than 3 involuntary exits have happened within the last 30s for this session, auto-respawn: call the existing `respawn()` method, spawn a fresh `pump_session_output` thread (exactly like the manual `"control"/"respawn"` path already does — reuse that code, don't duplicate it), after a short delay (0s for the 1st attempt, 2s for the 2nd, 5s for the 3rd — avoids hammering e.g. an SSH auth failure in a tight loop).
  3. If 3+ involuntary exits have happened within 30s, stop auto-respawning this session (it's crash-looping, not transient) and instead broadcast an augmented `exited` message marking it as needing a manual trigger.
- `exited` message gains two new optional fields (additive, backward compatible with the existing shape documented at the top of `main.rs`):
  ```
  {"type":"exited","session":N,"code":Option<i32>,
   "auto_respawning":bool,          // true if a server-side retry is already queued
   "needs_manual_respawn":bool}     // true once the 3-in-30s limit is hit
  ```

**Client side (`app.js`):**

- On `'exited'` with `needs_manual_respawn: true`, record `state.deadSessions[session] = true` (new state field) instead of (or in addition to) the current placeholder "use button 5 to respawn" text — rewrite that message to something accurate, e.g. `'\n[session exited — select this tab again to restart it]\n'`, since button 5 doesn't exist.
- `switchToSession(index)` (§1) gets one more check at the top: if `state.deadSessions[index]` is true, send `{"type":"control","session":index,"action":"respawn"}` (the existing message shape, zero protocol change) *in addition to* switching focus to it, and clear `state.deadSessions[index]`. This means the same physical button that already switches to that session (button 1 for session 0, etc. — no new button needed) doubles as "bring this session back" simply by virtue of pressing it again once it's known dead — matches the intuitive "press the tab for the thing that's broken to try it again" behavior and needs no new hardware trigger or WS message type.
- On `'exited'` with `auto_respawning: true`, no user action needed; could optionally show a transient "[session N restarting…]" line, cosmetic, skip unless requested.

No new WS message type is required for this requirement — `exited` is extended with two optional boolean fields, and the existing `"control"/"respawn"` action is reused as the manual recovery trigger.

---

## 9. Requirement 7: session 0 ("local-shell") runs `glances` instead of a login shell

### Decision

Once §1-§8 are implemented and deployed, change `terminal-agent-config.json`'s session-0 entry's `command` from the current shell invocation to:

```json
{ "name": "local-shell", "command": ["glances"] }
```

(keep the existing `name` field as-is unless the user wants it renamed to e.g. `"glances"` to match — cosmetic, not required).

### Notes / preconditions

- Bare `glances` with no flags is correct here: it launches glances' normal full-screen interactive curses UI directly in the PTY, which is exactly what a live, continuously-updating terminal session should show. This is a different use case from an earlier one-shot `--stdout-json` snapshot invocation — that flag is deliberately **not** used here.
- Precondition to verify before flipping this config, not assumed by this design: `glances` must actually be installed and on `PATH` for whatever user/environment `terminal-agent` spawns session 0's PTY as (root, per the task's stated daemon model). If it isn't yet present on the yocto-superbird image, that's a separate install step (opkg/pip/vendored binary, whichever this image's package story supports) — out of scope for this design, just flag it as a hard dependency of this one config change.
- No `terminal/agent` or `terminal/frontend` code changes are needed for this requirement — `command` is already a free-form `Vec<String>` per `SessionConfig`/`spawn_pty()`, exactly like the existing `ssh` session 1 entry; this is a pure config-file change, deployed last (after §1-§8 land and are verified), so that if anything regresses it's trivially attributable to the config change alone and not conflated with the button/knob/clock/respawn work.

---

## 10. Updated summary of exact new WS payloads (supersedes §5)

```
prototype/control-surface/agent -> frontend (ws://*:8765, unchanged shapes):
  {"type":"button","code":N,"value":V}   // now includes code 1 (KEY_ESC), unchanged
                                          // shape; ESC's actual restart action happens
                                          // locally in prototype/control-surface/agent, not via this message
  {"type":"knob","delta":D}              // unchanged, already implemented

terminal/agent -> frontend (ws://*:8766):
  {"type":"clock_sync","offset_ms":<i64>,"status":"ok"|"stale"|"unsynced",
   "age_s":<u64, present when status != "ok">}                 // new (§4)
  {"type":"exited","session":N,"code":Option<i32>,
   "auto_respawning":bool,"needs_manual_respawn":bool}          // extended (§8),
                                                                  // base shape pre-existing

frontend -> terminal/agent (ws://*:8766):
  {"type":"control","session":N,"action":"respawn"}             // pre-existing shape,
                                                                  // reused as the manual
                                                                  // dead-session recovery
                                                                  // trigger (§8), no new type
```

Everything else (session switching, scrolling, M/Enter passthrough, ESC hard-restart) is resolved without any new message type, either client-side in `app.js` from pre-existing bridge messages, or entirely inside `prototype/control-surface/agent` with no WebSocket round-trip at all.
