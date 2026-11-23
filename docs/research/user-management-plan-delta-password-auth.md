# Delta plan: password login for kiosk, no baked SSH key, first-boot password on an M-button screen

Created: 2026-11-23. Revised 2026-11-23 to the owner's final spec. Status: implemented, pending build (code, tests and docs are in the repo; no bitbake build and no device run yet). Unverified: package name `pam-plugin-faillock`, recipe parse and QA, flag passthrough, faillock line ordering, overlay boot ordering and device behaviour (M press interception, arrow position, key and password login, factory reset). See `docs/users.md`. Plan only: no code, no commit, no build, no flash. This file amends `docs/research/user-management-plan.md` (the plan) and `handover.md` (uncommitted, repo root). Where this file and the plan disagree, this file wins. Checked against the repo at HEAD `28f204c` (clean except the untracked `handover.md`). Evidence tags: V = read in a file or run read-only today, U = not verified.

This revision replaces the earlier toast design (100 s toast, knob-push reveal, last-four ack) entirely. Nothing of that design survives except where section 3 says "stays".

## 1. Owner spec (authoritative)

- S1. No SSH key is baked at build. The key flow is removed.
- S2. `kiosk` authenticates with a password, also over SSH. Root stays locked. `PermitRootLogin no`, no empty passwords.
- S3. If `kiosk` already has a usable password hash, generation is skipped entirely and nothing is shown.
- S4. Otherwise, at boot the frontend shows a full-screen menu-style message: the kiosk user password is unset, with an arrow pointing at the physical M button.
- S5. Pressing M once generates the password (controller, using today's provision core). The screen then REPLACES that message with the password itself. Not a toast, no 100 s timer, no timeout of any kind.
- S6. Pressing M again dismisses the screen and acknowledges. The second M press is the ack. No last-four proof.
- S7. A reboot while our own password was never acknowledged regenerates (the unset screen comes back and M generates a new one).
- S8. Faillock `deny=5 unlock_time=600`.
- S9. The owner accepts: an overlay failure leaves no remote access; password SSH gives single-factor remote root through kiosk sudo.
- S10. Phase 1 docs, Phase 2a (user and group menu) and Phase 2b (groups, SSH keys menu, keys optional for `kiosk`, sshd `AuthenticationMethods password publickey` so later keys work) are all implemented before the single real bitbake build at the end.

## 2. What this supersedes

- Plan D4 (`user-management-plan.md:102`, baked key as degraded-mode access) and D5 (`:103`, second `AuthorizedKeysFile` path for a baked key): gone. The menu-managed key store path survives for Phase 2b.
- Plan acceptance bullets about key login, refused password login, and degraded mode with the build key: rewritten in section 12.
- Plan section 16 (build key handling, `:402-404`) and the KEY_ONLY part of T-P1.3 (`:445`): deleted. The ADB and UART flag half (`:406`) stays.
- Plan D13 (ack by last four characters), plan section 8 `user_bootstrap_reveal` and `user_bootstrap_ack`-with-`last4`, and plan section 10 first table row (reveal, countdown, picker ack): replaced by sections 4 and 5.
- Plan section 12 first row ("Key-only auth ... no password to guess"): replaced by section 7.
- Plan `:376` and `:139` (key login works in degraded mode): deleted, see section 7.3.
- Handover "Decisions made" bullet 3 (`handover.md:13`), "Next steps" 1 (`:43`) and "Open items" 1 (`:53`): void. The devel housekeeping item (`handover.md:64-66`, `~/thing-revived-kiosk.pub`) stands.
- Handover `:47` and `:57-58` stay valid inputs, except that the three extra error codes it lists (`press_required`, `expired`, `not_pending`) are all removed by this design.
- The prior version of this delta (toast, `reveal_display_secs`, payload-less ack from a timer) is withdrawn.

## 3. What stays and what goes

Stays unchanged: `/etc` overlay on `/var` (G0), `kiosk` seed via extrausers with hash `!` (`superbird-kiosk-prod-image.bbappend:101-105`), root locked, `nullok` removed (`libpam_%.bbappend:237-255`), terminal agent as `kiosk` (`thing-revived-terminal-agent.service:15-16`), log policy `TypeOnly` for `user_*` events (`controller/src/app/hub.rs:55`), the generator and hashing core (`domain/accounts/password.rs`, `provision.rs` chpasswd path), ADB and UART flags.

Goes: `THING_REVIVED_SSH_PUBKEY_FILE`, `resolve_ssh_pubkey`, `KEY_ONLY`, `kiosk.authorized_keys` staging, the baked-key fetch guard, the baked `AuthorizedKeysFile` path, the `baked` key row and field in Phase 2b, the baked-key checker rules, the last-four picker and `ack_*` limiter and config, the knob-push `PressGate` and `PressTap` and `reveal_press_window_secs`, the tmpfs secret holder and its 900 s expiry (`first_boot_ttl_secs`), the `user_bootstrap_reveal` and `user_bootstrap_ack` commands, `expires_in_secs` on both bootstrap events, the error codes `bad_ack`, `press_required`, `expired`, `not_pending`.

## 4. First-boot flow (controller semantics)

### 4.1 Decision at boot (pure function in `domain/accounts/state.rs`)

Inputs: the `kiosk` hash row of the current `/etc/shadow` (usable means non-empty and not starting with `!` or `*`; read through `AccountFiles::read_current`, V used in fakes at `services/users.rs:499`) and `state.json` (`acknowledged`, written by us, or absent).

| kiosk hash | state.json | Action | Screen |
|---|---|---|---|
| usable | absent | Adopt: write `acknowledged=true`. Covers a hash set by hand, a `/var` restored from backup, a lost state file. Never generate. | none |
| usable | `acknowledged=true` | Steady. | none |
| usable | `acknowledged=false` | Our own password that was generated and never acknowledged. Re-lock the hash to `!`, keep state unacknowledged. | unset message |
| unusable (`!`, `*`, empty) | any | Nothing generated yet. | unset message |

- The re-lock in row 3 is a deliberate addition (S7): the unacknowledged password may have been seen on screen by someone, and the unset message must be true. Implemented in `provision` at boot, before the controller starts, with the existing account adapter (U: the exact write is `usermod --password '!'` or a shadow row edit, D-C2a decides and tests).
- Owner statement S3 ("already has a usable hash, skip") is honoured literally for rows 1 and 2. Row 3 is the only place where a usable hash does not block, which is exactly S7.
- Storage failure (overlay down, `StorageUnavailable`, `provision.rs:82-86`): no generation, `storage_ok=false`, the frontend shows "Account storage is not available" instead of the unset message and M is not intercepted (section 5.5).

### 4.2 Phases (controller owns them)

`none` (nothing to show) -> `prompt` (unset message) -> `generating` (M pressed, work in flight) -> `showing` (password on screen) -> `done`. A failure goes back to `prompt` (generation) or stays in `showing` (ack write) with an error code on the next event.

- `prompt`, first M press: start generation on a worker thread, never in the evdev loop. Phase `generating`; further M presses are consumed and ignored until it ends.
- Generation order (changed to close a lockout hole): generate in memory, write `state.json` `acknowledged=false` FIRST, then `chpasswd`, then enter `showing`. If power is cut between the two writes, the next boot sees a usable hash with unacknowledged state (row 3), re-locks and prompts again. The old order (hash set, then state) would have let row 1 adopt a never-shown password as owner-set, which is a permanent lockout. This is invariant I1, test it with a fake that fails after each step.
- The plaintext lives only in controller memory (`Secret`, zeroised on drop where the type supports it). No tmpfs file at any time, so `/run/thing-revived/first-boot-secret` and the 900 s hold are gone. The port `SecretStore` stays with an in-memory implementation to keep the service shape and tests; `secret_tmpfs.rs` and `[users] secret_dir` are removed if nothing else uses them (D-C2a decides, `layering` test must stay green).
- `showing`: the secret is held until the ack, with no timer. It is not in `state.json`, the journal, argv, or the build output. The only persisted artefact is the sha512 hash in the overlay's `/etc/shadow`.
- `showing`, second M press: accepted only if (a) at least one frontend connection received `user_bootstrap_secret` (delivered), (b) at least `ack_min_gap_ms` (default 1500) passed since delivery. Then write `acknowledged=true`, scrub memory, phase `done`, broadcast. The gap is an input debounce against contact bounce and a double tap that would dismiss the password before it was read. It is not a display timeout. A press that fails (a) or (b) is consumed and ignored.
- Chromium or the ws connection drops while `showing`: no ack is possible until a frontend is back (section 5.4 `ready`). On reconnect the frontend asks again and receives the same secret again (still in memory). No forfeit, no regeneration.
- Controller process restart while `generating` or `showing`: memory is lost, state is unacknowledged, the controller recomputes the table in 4.1 at start. A usable-but-unacknowledged hash with no live secret is treated like row 3 in the controller too (re-lock then prompt) so a controller crash cannot strand an unseen password.
- Generation failure (chpasswd error, storage): phase back to `prompt`, `user_error` code `generate_failed`, state left unacknowledged. Ack write failure: stay `showing`, `user_error` code `ack_failed`, M retries.
- After `done` the password is final: S3 applies from then on. Phase 2a `user_set_password` on `kiosk` is the way to change it (it changes the SSH password too).

### 4.3 Reboot semantics (S7)

Reboot before the second M press: table row 3 (or row 4 if lock-only), unset screen returns, M generates a new password. Reboot after the ack: row 2, nothing shown. A reboot after the ack with a lost `state.json` and the hash intact: row 1, adopted, nothing shown.

### 4.4 Alternatives considered

- Generate at boot (today's `provision`) and only reveal on M. Ruled out by S5 wording and by the property that an unseen reboot then leaves a live password nobody saw. The invariant I1 plus the re-lock achieve the same safety with generation at the press.
- Frontend generates or sends the password to set. Ruled out: a local process can speak the same websocket (`ws://127.0.0.1:8765`).
- Keep the tmpfs holder so a controller crash can resume the display. Ruled out: it keeps plaintext at rest for no benefit given S7 (regenerate is acceptable).
- Steelman against the recommendation: the ack is a physical button press that the controller cannot prove was a human reading the screen, and the delivered-secret check cannot prove Chromium actually rendered it. Both only risk the owner dismissing early; recovery is the factory reset in section 7.3.

## 5. M button interception

### 5.1 What exists (V)

- The buttons evdev device (`/dev/input/event0`) is grabbed exclusively with EVIOCGRAB (`controller/src/adapters/evdev_input.rs:30-39`, not `main.rs`; `run_buttons` is spawned from `start_input` in `controller/src/lib.rs:100-116`). A grab failure only logs an error and continues: raw keydowns then leak to Chromium (`evdev_input.rs:35-37`).
- Every key event goes to `ButtonPolicy::handle` (`controller/src/services/input.rs:42-69`): (1) broadcast `BridgeEvent::Button{code,value}` to all websocket clients unchanged, (2) if `is_replayed(code)` (exactly `KEY_M`=50 and `KEY_ENTER`=28, `domain/keys.rs:12-14`) replay it through the uinput virtual keyboard, so Chromium sees a normal DOM keydown, (3) for `KEY_M` press (value 1) start the long-press timer thread, which restarts `restart_unit` (the terminal agent) when the hold crosses `longpress_ms` while still held; value 0 cancels.
- So today an M tap types a literal `m` into the PTY: uinput `m` -> DOM `keydown` -> `frontend/src/input/keyboard.js:41-55` -> `agent.sendInput` (`boot.js:184-187`), unless the menu is open (`isMenuOpen`, swallowed with `preventDefault`). Knob push (`KEY_ENTER`) is replayed the same way and also arrives as bridge `button` code 28. Buttons ESC and 1-4 exist only as bridge `button` events (`frontend/src/utils/types.js:83`); M (50) is not in `ButtonCode`, so the chart ignores its bridge event.
- The knob push is now free: the `PressTap`/`PressGate` knob reveal (`services/users.rs:32-79`) is removed (section 3).

### 5.2 Decision: the controller decides, at the evdev reader

The physical press is the only authority (a websocket client or a BT keyboard cannot forge a KEY_M on the grabbed device). So interception lives in `ButtonPolicy`, with a new port `FirstBootGate` (trait in `ports.rs`, in-memory implementation in `services/users.rs`) injected through `ButtonPolicy::new`:

- `fn active(&self) -> bool`: true only in phases `prompt`, `generating`, `showing` AND a frontend is `ready` (5.4).
- `fn on_m_down(&self)`: called once per consumed M press; the implementation advances the phase machine of section 4.2 (spawning the worker for generation, or doing the ack).

`handle` changes, in this order, for each event:

1. Always broadcast the raw `Button` event first (unchanged, so the frontend still sees every code and existing tests keep their meaning).
2. If `code == KEY_M`: on value 1, `consumed_m = gate.active()`; if consumed call `gate.on_m_down()`. If `consumed_m` is true then for value 1, 2 (autorepeat) and 0 skip steps 3 and 4 entirely and clear `consumed_m` on value 0. If `consumed_m` is false the existing path runs unchanged (replay, timer, cancel). The decision is therefore keyed to the press edge, never re-evaluated mid-hold.
3. If `code == KEY_ENTER` and `gate.active()`: do not replay (the knob push must not type `\r` into the PTY under the overlay). The same press-edge rule applies (`consumed_enter`, so a release without its press is not replayed either).
4. Existing replay, timer and warn paths for everything else, unchanged.

Consequences to keep true and test:

- Long press (hold >= `longpress_ms`) while the screen is active does NOT restart the terminal agent and does NOT trigger a second ack: the held key is one logical press on its down edge, autorepeat is ignored. Long press restart works exactly as today when the gate is inactive.
- A press that began while inactive (typed `m`, timer running) and ends while active (the phase flipped meanwhile, for example `ready` just became true) is not consumed on release: the release is replayed and cancels the timer, so no stuck key and no spurious restart. The reverse (press consumed, then gate inactive at release, for example the second press acking) suppresses the release: no orphan `m` up event reaches uinput.
- Only the controller's own evdev M can move the phase machine. There is no websocket command that generates or acknowledges (both removed in section 3). The websocket carries state out and the frontend's readiness in (`user_bootstrap_get`), nothing else.
- `generating` consumes M presses silently. `none` and `done` never consume.

### 5.3 Frontend gating: every keystroke and input path

The controller gate stops the uinput replay, but the overlay must be safe even if the grab failed (raw keys reach Chromium, `evdev_input.rs:35-37`), a BT keyboard is attached (a separate, ungrabbed evdev node), or the screen is touched. D-F1 gates all of the following while the first-boot overlay is visible (a single predicate `fbActive()` derived from the `user_bootstrap` phase being `prompt`, `generating` or `showing`):

- `frontend/src/input/keyboard.js` `bind`: swallow every keydown in the capture phase with `preventDefault`, `stopPropagation` and `stopImmediatePropagation`. Wire `boot.js:186` as `isMenuOpen() || fbActive()`. A raw `m` (grab failure) or BT keyboard `m` does nothing and does not count as the M press, because only the controller counts presses.
- The single sink `agent.sendInput` (`boot.js:187`; xterm `onData` is not used, `terminal-view.js:5`): wrap so it drops all bytes while `fbActive()`. Second line of defence for any path that bypasses `keyboard.js` (for example the on-screen text field effect).
- The interaction chart (`menu-chart.js`): while `fbActive()` ignore bridge `button` events for ESC, 1-4 and Enter, and knob deltas. ESC must not open the menu over the password; 1-4 must not switch sessions; a knob push must not select anything. The screen is not dismissible by any control except the M press.
- Touch: the overlay is `position: fixed; inset 0` equivalent (no `inset`, see Chromium note below), highest z-index, `pointer-events: auto`, swallows `touchstart/touchend/click`, and `focus.js` `bindRefocus` (`:30-45`) and `bindTextField` become no-ops while active (no terminal refocus, no on-screen keyboard, no toast-through). Weston's on-screen keyboard must not be invokable (the text field is `disabled` while active).
- Toasts: ordinary toasts (Wi-Fi, Bluetooth, `sendFailed`) render below the overlay and cannot replace the password, because the password is not a toast.
- Visibility and wipe: the password exists only in a module-private variable and in the overlay DOM, never in `store`, the log, `localStorage`, `ui.users.*` or the reducers (the old contract of `users.js:8-11` is kept and tightened). It is wiped from variable and DOM on `done`, on websocket close, and on a `user_bootstrap` event whose phase is not `showing`. Reload starts empty and re-asks.
- CDP: any local process can read the rendered DOM over `--remote-debugging-port=9223` (`docs/research/chromium-147-frontend-audit.md:7,17`, V). The overlay is not protected from local processes. Do not claim otherwise in docs. Accepted with the existing Chromium hardening item (`handover.md:14`).

### 5.4 Readiness: when the gate is armed

`ready` means at least one websocket client has sent `user_bootstrap_get` since it connected and is still connected (the router needs a `connection_closed(conn)` hook, which `handover.md:54` says does not exist: `ws_server.rs` only calls `hub.unregister`; D-C2b adds it). Before ready, M behaves normally (types `m`), because nothing is on screen to intercept. If the last ready client drops during `showing`, the gate goes inactive (no unseen ack), and re-arms on the next `user_bootstrap_get`. The first M press never generates without a frontend that can show the result.

### 5.5 Not intercepted

`none`, `done`, and storage failure (`storage_ok=false`): M and Enter behave exactly as today. In storage failure the screen shows the storage message (not an unset message), nothing is generated, so M has nothing to do.

### 5.6 Wire (replaces plan section 8 Phase 1)

- Commands: `user_bootstrap_get` only (frontend is ready; reply is targeted). `user_bootstrap_reveal` and `user_bootstrap_ack` are deleted (fixtures `command.user_bootstrap_reveal.json` and `command.user_bootstrap_ack.json` deleted).
- Events: `user_bootstrap` (broadcast on every phase change and as the targeted reply to `get`) with fields `phase` (`none|prompt|generating|showing|done`), `username`, `storage_ok`; no `expires_in_secs`, no `pending` or `acknowledged` booleans (derive from `phase`). `user_bootstrap_secret` (`username`, `password`, grouped by the frontend, no `expires_in_secs`) is sent targeted to each ready connection when `showing` is entered, and to a connection that sends `user_bootstrap_get` while `showing`. Golden fixtures `event.user_bootstrap.json`, `event.user_bootstrap.none.json` (rename to the per-phase variants D-C1 chooses), `event.user_bootstrap_secret.json`, `event.user_error.json` (example code `generate_failed`) are updated byte for byte.
- Error codes for this flow: `generate_failed`, `ack_failed`, `storage_unavailable`. `bad_ack`, `press_required`, `expired`, `not_pending` leave the code list (plan `:321`, frontend `users-logic.js:27`).
- Residual (accept, list in docs): while `showing`, any local process that connects and sends `user_bootstrap_get` is handed the password, because the old press gate cannot apply when the trigger is a controller-side button. Same exposure class as CDP. The Origin check (`29d6836`) stops browsers on other origins, not local programs.
- Logging: unchanged policy, both bootstrap events are `TypeOnly`; `no_secret_logging.rs` and the golden "no password outside the two secret events" test stay.
- Config `[users]`: remove `first_boot_ttl_secs`, `reveal_press_window_secs`, `ack_free_attempts`, `ack_backoff_max_secs`, `secret_dir` (if removed in 4.2); add `ack_min_gap_ms = 1500`. Config tests require TOML equal to defaults, so `config/controller.toml` and the compiled defaults change together.

## 6. Frontend design (single owner task D-F1)

- New dedicated overlay element `#first-boot` in `frontend/index.html`, styled with the existing menu look (reuse menu class names where the screen markup allows), above the menu and toast layers. Not part of the menu screen stack, so no menu route can reach or dismiss it. The `users-bootstrap` and `users-ack` menu screens, the root-menu return row, `ackApply`, `ackDisplay`, `ACK_LEN`, `bad_ack` text, `secret-view.js` ack views and their tests (44 ack and last4 hits in `frontend/tests/users.test.js` plus `frontend/tests/e2e/menu-regression.mjs`) are deleted or rewritten.
- `prompt` view: title "Kiosk password not set", body text "The kiosk user password is unset. Press the M button to generate one.", and an arrow pointing at the physical M button. Arrow position is a CSS constant (U: the physical position of M relative to the 800x480 panel must be read from `docs/hardware.md` or a device photo and checked in the single flash).
- `generating` view: "Generating..." with no arrow (the press is consumed and nothing else is accepted).
- `showing` view: the password in four groups of four, large monospace, `user-select: none`, not an input, a line "Save this password now. It will not be shown again.", and a second line "Press M again to confirm you saved it". No countdown, no timer anywhere on this screen. The alphabet already excludes `l I 0 O` (`domain/accounts/password.rs:8`).
- Error line for `generate_failed` and `ack_failed` (message from the event, never the secret).
- Syntax: classic scripts, Chromium 68/69 ceiling kept through `syntax-guard.test.js` (no `?.`, `??`, flex `gap`, `inset`, `:focus-visible`, modules). The guard is not modernised in this delta (Q12).
- Tests: node unit tests for the reducer-free module (phase view, wipe on done, close, non-showing), `keyboard.test.js` (swallow when active), chart test (ESC and knob ignored when active), e2e in `menu-regression.mjs` and `mock-server.mjs` including an assertion that neither the DOM nor the console contains the password after `done`, and a second-press-without-delivery case. Run `render()` in node and e2e with a Wi-Fi toast arriving during `showing` (U if the overlay layers correctly in Chromium; verify in e2e where a browser exists, else flag).

## 7. Security consequences of password SSH

### 7.1 sshd shape (replaces `10-thing-revived-hardening.conf`, same first-value-wins layout)

```
PermitRootLogin no
PasswordAuthentication no
PermitEmptyPasswords no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AuthenticationMethods publickey
UsePAM yes
AuthorizedKeysFile /var/lib/thing-revived-ssh/authorized_keys/%u
AuthorizedKeysCommand none
MaxAuthTries 3
LoginGraceTime 30
X11Forwarding no
PermitUserEnvironment no
Match User kiosk
    PasswordAuthentication yes
    AuthenticationMethods password publickey
```

- Space separated `password publickey` means alternatives in sshd, so `kiosk` may log in with the password OR a key from the store, and any key added later through Phase 2b works for `kiosk` without another image change (S10). Other users stay key-only. Root and non-kiosk password login stay refused.
- `KbdInteractiveAuthentication no` stays: with `UsePAM yes` the `password` method still goes through the PAM auth stack, so faillock applies.
- Alternatives considered: global `PasswordAuthentication yes` plus `AllowUsers kiosk` (ruled out: breaks the key-only default for Phase 2a/2b users); `AuthenticationMethods password` only (ruled out by S10); `AllowGroups` (no new information).
- Socket limits stay (`MaxConnections=16`, `MaxConnectionsPerSource=3`, `thing-revived-sshd-socket.conf:42-43`). Optionally lower `MaxConnections` to 8; owner-visible, default keep 16.
- `MaxAuthTries 3` is per connection and sshd runs per connection under `sshd.socket` `Accept=yes` (`:39-40`), so `MaxStartups` and `PerSourcePenalties` do not apply.

### 7.2 Faillock (S8)

- `pam_faillock.so` and `/usr/sbin/faillock` are built by libpam 1.7.2 (V, devel build tree) but not installed by default; `libpam_1.7.2.bb:58,112-117` splits `pam_*.so` into dynamic `pam-plugin-*` packages (package name `pam-plugin-faillock` is U until `bitbake -p`).
- Lines in `common-auth`, inline options (no `faillock.conf`, so no collision): `auth required pam_faillock.so preauth deny=5 unlock_time=600 fail_interval=900`, then the existing `pam_unix.so` line, then `auth [default=die] pam_faillock.so authfail deny=5 unlock_time=600 fail_interval=900`, `auth sufficient pam_faillock.so authsucc`, and `account required pam_faillock.so` in `common-account`. `fail_interval=900` is faillock's own default made explicit. Exact ordering against the existing `[success=1 default=ignore]` line is U, validated in the single build and on device (Risk 3).
- The counter lives in tmpfs `/var/run/faillock`, so a reboot resets it. Cost: it is per account, so five wrong guesses per ten minutes from the network lock `kiosk` out of SSH, `sudo` and `su` (bounded denial of service). Key logins for `kiosk` (Phase 2b) are not counted and not blocked by it.
- Interaction: the controller verifies admin passwords through `unix_chkpwd` directly (plan `:288`), so menu unlock attempts are not counted by faillock and are limited by the controller's own limiter. A faillock lock does not block menu unlock. State this asymmetry in docs.

### 7.3 Accepted by owner (S9)

- Single-factor remote root: the `kiosk` password through `sudo` is root over SSH. Generated start password is 16 characters from a 56 symbol alphabet, about 93 bits (`password.rs:8,10`); the weak point is any human-chosen password set later in Phase 2a (minimum 12 characters). Per-guess cost 0.17 s (`user-management-spikes.md:215`).
- Degraded mode: if the `/etc` overlay fails, `provision` returns `StorageUnavailable`, no password is set, `kiosk` stays `!`, remote login is impossible and `sudo` cannot succeed. The screen still runs the terminal as `kiosk`. Recovery is the armed factory reset from a PC in burn mode (plan section 13, `:373-376`, which also wipes Wi-Fi and Bluetooth) or a reflash. Delete the plan's "key login works in degraded mode" claim.
- After the ack, a lost password is a lockout with the same recovery. Say it loudly in `docs/users.md`.
- Journal: failed attempts name the target user, never the password. A user who types a password at the username prompt leaks it to the journal that `adm` can read. Low risk, document.
- Residuals to list in the threat model: the one secret gives root; a lockout denial of service by faillock; CPU load from guesses (Weston and Chromium share the SoC); Chromium root plus the CDP port expose the overlay text locally; the websocket hands the secret to any local connector during `showing`; anyone who can reboot the device before the ack sees a fresh password on the screen (physical access already defeats the device, plan `:364`).

## 8. Checker, build script and image wiring

### 8.1 `build/tools/check_image_accounts.py` (owner D-K1)

- Remove the baked-key rule: `PATHS["baked_key"]` (`:40`), `check_baked_key` baked-key and build-key parts (`:551-579`), the `build_key` parameter of `run_checks` (`:745,750,782`), the `--build-key` argument (`:774`), and `PUBKEY_TYPES` and `parse_pubkey_line` (`:534-548`) if unused. Keep the private-key-marker scan over `SECRET_SCAN_DIRS` (`:105-110`) as its own `check_no_private_keys`.
- New rule: `/usr/share/thing-revived/accounts/kiosk.authorized_keys` must not exist and no `*.authorized_keys` under `/usr/share/thing-revived`.
- `SSHD_EXPECTED` (`:48-61`): `authorizedkeysfile` is the single store path, global `passwordauthentication` stays `no`, add `maxauthtries <= 3`.
- Match handling: `parse_sshd` (`:397-425`) and `check_sshd` (`:444-452`) refuse every `Match` today. Accept exactly one `Match User kiosk` containing only `PasswordAuthentication yes` and `AuthenticationMethods password publickey` (order of the two words must be exactly that). Fail on any other Match, a Match for another user, an extra key inside it, `PasswordAuthentication yes` outside it, or an override of `PermitRootLogin` or `PermitEmptyPasswords` inside it.
- New PAM rule: `common-auth` has the three faillock lines with `deny=5` and `unlock_time=600`, `common-account` has `account required pam_faillock.so`, and `pam_faillock.so` exists in the rootfs. `check_pam` (`:611`) keeps the no-`nullok` rule. Add the ipk name to `IPK_REQUIRED` (`:123`) only after the name is confirmed (U).
- Accounts: `kiosk` still ships hash `!`; add a rule that no account ships a real hash.
- Unchanged: root locked, no empty hash, no second uid 0, sudoers, securetty, serial getty and ADB gates, unit wiring, free space.
- `rootfs_selftest.sh:14-16`: remove `--build-key`. Optionally add `sshd -T -C user=kiosk,host=h,addr=1.2.3.4` and `-C user=root,...` on a copy.
- Fixtures: update `fixtures/good/etc/ssh/sshd_config.d/10-thing-revived-hardening.conf` and `fixtures/good/etc/pam.d/common-auth` (and `common-account`); delete `fixtures/good/usr/share/thing-revived/accounts/kiosk.authorized_keys` and `fixtures/build-key.pub`; remove bad fixtures `key_empty`, `key_missing`, `key_wrong`, `key_with_options` (expected strings at `test_check_image_accounts.py:46-49`); add `key_baked_present`, `password_global`, `match_other_user`, `match_extra_key`, `kiosk_authmethods_wrong`, `faillock_missing`, `faillock_so_missing`, `maxauthtries_high`; refresh `sshd_authkeys_path`, `sshd_password_auth_early`, `sshd_match_block` (an unauthorised Match), `sshd_kbdinteractive`, `pam_nullok`, `kiosk_has_hash`. Drop `BUILD_KEY` from the test file and add a CLI test that `--build-key` is rejected.

### 8.2 Build script and docs wiring

- D-B1 `build/build-image.sh`: delete `SSH_PUBKEY_FILE`, `KEY_FILE_NAME`, `SSH_PUBKEY_LINE`, `resolve_ssh_pubkey` (`:105-152`), the `KEY_ONLY` block (`:292-295`), the call (`:302`), the staging and trap block (`:380-395`), header lines `:19,24`, the CONFIG_ONLY key note. `build/test-build-image.sh`: delete `key_only` and key cases (`:19-57`), keep the flag and kas-content tests. `build/README.md:23,30-41` and the `flash-image.sh` success message (from `9647626`, "SSH is key-only ... key baked in at build time"): rewrite to password login. `.gitignore:19-20`: keep `*.authorized_keys` defensively, drop the `kiosk.` line.
- D-B2 accounts recipe `build/overlay/recipes-extended/thing-revived-accounts/**`: new sshd drop-in per 7.1; `.bb` drops `file://kiosk.authorized_keys` (`:61`), the `do_fetch:prepend` guard (`:83-97`), the key install (`:118-120`, keep the share dir and `build-flags`), and the DESCRIPTION sentences (`:45-50`); rewrite the `thing-revived-provision.service` comments (`:23-25`) and update the provision unit if the boot decision of 4.1 needs the controller to start after it. Remove `RuntimeDirectory` entries used only by the tmpfs holder if D-C2a drops it (coordinate through the orchestrator, D-B2 owns the unit).
- D-B3 image and PAM `build/overlay/recipes-core/images/**` and `build/overlay/recipes-extended/pam/**`: add the faillock plugin to `IMAGE_INSTALL`; extend `thing_revived_assert_accounts` (`:177-220`) with: no `*.authorized_keys` under the share dir, sshd drop-in has `Match User kiosk` and global `PasswordAuthentication no`, `common-auth` carries `pam_faillock.so` and no `nullok`; keep the kiosk `!` assertion (`:188-189`). `libpam_%.bbappend`: add the faillock lines in the `do_install:append` that strips `nullok` (`:237-255`) with a `bbfatal` that checks them.
- ADB and UART logic and `kas/` flag file: untouched.

## 9. Phase 2a and 2b deltas

- 2a unlock and add flows unchanged in design. The admin unlock password equals the SSH password; the on-screen keyboard is the entry path (plan `:342`). `user_set_password` on `kiosk` stays allowed and changes the SSH password too; its menu text says so. Controller limiter and faillock are separate (7.2). 2a must not touch the sshd drop-in or faillock files.
- 2b: groups unchanged. SSH key menu ships now (Q9), minus the baked key: no read-only baked row, no "removing the baked key is refused" check (plan `:501`), no `baked_key_present` reconcile warning (`provision.rs:140-142`), `baked` removed from `ssh_key_list` and its golden fixture and `types.js` (Q11). Keys for `kiosk` work through the widened Match of 7.1; keys for other users work under the key-only default. `key_count` in `user_list` stays. Gate G2b-d loses its baked-key sentences and gains "add a key for `kiosk`, log in with the key, then with the password, both work".
- The key store path is `/var/lib/thing-revived-ssh/authorized_keys/<n>` (sshd `AuthorizedKeysFile`). With no baked key, `kiosk` has no store file until the menu adds one; sshd then falls back to password for `kiosk`.

## 10. Resolved open questions (all 13)

1. Regeneration versus skip: both, by the table in 4.1. Skip (adopt) when a usable hash exists and is not our own unacknowledged one; regenerate on reboot while our own password is unacknowledged (owner S3, S7).
2. Press gate versus automatic: the unset screen appears automatically at boot; generation waits for the physical M press, which is the press gate (S4, S5).
3. What counts as seen: the second M press (S6), guarded by "secret delivered to a ready frontend" and a 1.5 s debounce. No last-four, no timer ack.
4. Degraded mode: accepted, no dev escape key (S9, 7.3).
5. Faillock: `deny=5 unlock_time=600`, `fail_interval=900` made explicit (S8), bounded lockout denial of service accepted.
6. sshd shape and key policy: `Match User kiosk` with `PasswordAuthentication yes` and `AuthenticationMethods password publickey`; other users key-only (S10).
7. Single-factor remote root: accepted (S9).
8. Toast mechanics: no toast. One protected full-screen overlay, not part of the menu stack and not the toast slot (the toast system stays untouched, 12 px and 4 s maximum rules out any reuse). Legibility on 800x480 verified on the device in the single flash.
9. Phase 2b key menu: ships now with groups (S10).
10. Timers: none. The 900 s hold and the 100 s display are gone; `ack_min_gap_ms = 1500` is a debounce, not a timeout. Range 500 to 5000.
11. `baked` field: dropped from wire, fixtures and `types.js`.
12. Frontend language ceiling: modernise nothing, keep `syntax-guard.test.js` and Chromium 68/69 syntax in new code.
13. Developers: password only, no dev key flag. Dev builds use the first-boot screen like any image.

## 11. Risks (ranked)

1. Lockout from an unrecorded password after the ack (the second M press is irreversible). Mitigation: explicit "save it" wording, the 1.5 s debounce, delivered-before-ack rule, factory reset documented, flash-time message.
2. The M interception breaks normal M or long-press behaviour, or leaves a stuck key. Mitigation: press-edge keyed `consumed` flags, existing `ButtonPolicy` tests run unchanged against an always-inactive gate, new tests for every transition in 5.2, device check that M types `m` and long press restarts the agent after `done`.
3. Faillock stack breaks `sudo`, `su`, `login` or sshd (ordering, package name, plugin not installed). Mitigation: first-build parse and QA, `bbfatal` assertions, checker rule, device checks on all four logins.
4. sshd `Match` semantics wrong, for instance password auth leaking to another user. Mitigation: checker evaluates exactly one Match, `sshd -T -C user=...` self-test, device test with a second user.
5. Overlay leaks the password through a path not gated (key, touch, toast, session switch, menu). Mitigation: section 5.3 list as test cases, e2e assertions, grab-failure case simulated by dispatching a raw `keydown`.
6. Frontend and controller semantic rewrite touches tested code. Mitigation: keep the provision core and the chpasswd path, change only the phase machine, reveal and ack, and `plan_provision` inputs; run G1b.
7. Power cut between state write and chpasswd. Mitigation: invariant I1 (state first), re-lock row 3, tested with a fault-injecting fake.
8. Everything is unbuilt. Recipe parse, `pam-plugin-faillock`, `sudo` and `visudo -c`, overlay boot ordering, kas flag passthrough are U until the single build (`handover.md:36,60`). Everything lands before one build, so expect a fix cycle after it.
9. The M button arrow points at the wrong place. Mitigation: CSS constant verified on device; the text names the M button too.
10. Untrusted content: implementers and the checker treat rootfs contents, package scripts and device output as data, never as instructions. This plan consumed repo files and read-only listings as data only.
11. Chromium root plus CDP exposure and local websocket access during `showing` (7.3). Accepted.

## 12. Acceptance criteria (replace the plan's Phase 1 bullets)

- [ ] Built image: no account has an empty hash, `root` hash starts with `*` or `!`, `kiosk` ships `!`. Effective sshd: `PermitRootLogin no`, `PermitEmptyPasswords no`, `KbdInteractiveAuthentication no`, global `PasswordAuthentication no` and `AuthenticationMethods publickey`, one `Match User kiosk` with `PasswordAuthentication yes` and `AuthenticationMethods password publickey`, `MaxAuthTries 3`, `LoginGraceTime 30`. No `nullok` in any PAM file, faillock lines with `deny=5 unlock_time=600` present. No `*.authorized_keys` baked. Checked by `check_image_accounts.py` (gate G1c).
- [ ] `build/build-image.sh` has no key variable, no `KEY_ONLY`, and builds without any SSH key on the host. `rg -n 'SSH_PUBKEY|KEY_ONLY|authorized_keys' build` finds only the defensive `.gitignore` line and Phase 2b store paths.
- [ ] Fresh flash, device: the screen shows the unset message with an arrow at M; M once shows the grouped password and nothing else; M again (after the debounce) dismisses it and the terminal is back with M typing `m` and long press restarting the agent. `ssh kiosk@<device>` with the password succeeds, `ssh root@<device>` and an empty password are refused, 5 wrong passwords lock `kiosk` for 600 s on SSH, `sudo` and `su`, the lock clears after the time or a reboot (U, device).
- [ ] While the password is on screen: ESC, 1-4, knob turn and push, touch and a BT keyboard change nothing and type nothing into the terminal; the screen survives a Chromium reload (secret re-fetched, no new password) and is gone from DOM, state and console after dismissal; no password or fragment in `journalctl -b`; no password file under `/run`, `/var` or `/tmp`.
- [ ] Reboot before the second M press shows the unset message again and M generates a different password; reboot after the ack shows nothing. A kiosk with a pre-existing usable hash (set by hand, state file removed) gets no screen and no generation (S3). Power cut test between state write and chpasswd (owner Class C) lands in the prompt, never in an adopted unseen password.
- [ ] Unit tests, golden fixtures, frontend node tests, e2e, checker tests and the build-script test pass where they can run. Device checklists G1d, G2a-d, G2b-d run only after the single build and owner-approved flash.

## 13. Task breakdown (disjoint file ownership, waves, gates)

Rule inherited from the plan: a task owns its files exclusively; if it needs another's file it asks the orchestrator. Within a wave no two tasks share a file. A file may change owner between waves (the earlier wave merges first). Delta task ids are `D-*`. The plan's Phase 2a and 2b tasks keep their ids and are run as merged pairs (`T-P2a.n` and `T-P2b.n` as one task under the id `T-P2.n`, full contents in plan `:468-503` with section 9 changes) because all of it lands before one build and the pairs share files. The key half of each T-P2.n is separable. No task builds, flashes, commits or pushes.

### Wave A (parallel, no cross dependencies)

- D-B1 (S) build script. Owns `build/build-image.sh`, `build/test-build-image.sh`, `build/flash-image.sh`, `build/README.md`, `.gitignore`. Section 8.2 first bullet. Done when `CONFIG_ONLY=1 build/build-image.sh` and `build/test-build-image.sh` pass and the grep in section 12 is clean.
- D-B2 (S) accounts recipe. Owns `build/overlay/recipes-extended/thing-revived-accounts/**`. Section 7.1 drop-in is final text, `.bb` cleanup, provision unit comments.
- D-B3 (M) image and PAM wiring. Owns `build/overlay/recipes-core/images/**`, `build/overlay/recipes-extended/pam/**`. Section 7.2 lines, `IMAGE_INSTALL`, assertions. Contract: `pam-plugin-faillock` (U, may be renamed after the first parse).
- D-C1 (M) protocol, all phases. Owns `controller/src/protocol/**`, `config/protocol/controller/**`, `controller/tests/wire_golden.rs`, `controller/src/utils/types.rs`. Section 5.6 plus the plan section 8 Phase 2a and 2b wire with section 9 changes (no `baked`). Done when `wire_golden` lists every fixture and no stale fixture remains.
- D-F1 (L) frontend, Phase 1 overlay. Owns all of `frontend/**` (src, tests, `index.html`, CSS) and `config/frontend.toml`. Sections 5.3 and 6. Non-wire parts start at once; the types and bridge-client tests finish after D-C1.
- T-P2.1 (M) accounts domain, pairs T-P2a.1 and T-P2b.1. Owns `controller/src/domain/accounts/{names,model,guard,ratelimit,session,events,groups,keys,mod}.rs`. Pure code and unit tests only. Does not touch `state.rs` or `password.rs`.

### Wave B

- D-C2a (L) provision core and boot decision. Owns `controller/src/domain/accounts/{state,password}.rs`, `controller/src/provision.rs`, `controller/src/main.rs`, `controller/src/ports.rs`, `controller/src/adapters/{accounts_shadow,secret_tmpfs,os_entropy,mod}.rs`. Section 4.1, 4.2 phase state machine and invariant I1 as pure code, the re-lock in `provision`, `FirstBootGate` and in-memory `SecretStore` ports, removal of `last4_matches` and the tmpfs holder usage. Depends on nothing in wave B; needs D-C1 merged only for type names, so it can read D-C1's output. Gate G1b slice: `cargo test` and `cargo clippy -- -D warnings` for these modules.
- D-K1 (M) checker. Owns `build/tools/**`. Section 8.1. Depends on D-B2 and D-B3 final strings. Gate: unit tests pass on the good fixture and fail on each new bad fixture.

### Wave C

- D-C2b (L) bootstrap service, M interception, wiring. Owns `controller/src/services/{users,input,mod}.rs`, `controller/src/app/**`, `controller/src/adapters/{ws_server,evdev_input}.rs`, `controller/src/lib.rs`, `controller/src/utils/config.rs`, `config/controller.toml`, `config/README.md`, `controller/tests/{layering,no_secret_logging,router_flow,log_policy}.rs`. Sections 4.2, 5.1 to 5.6: `ButtonPolicy` gate and `consumed` flags, phase service, worker thread, delivered and debounce rules, `connection_closed` hook, removal of `PressGate`/`PressTap`/`AckLimiter`/ack handler, config key changes. Depends on D-C1, D-C2a. Gate G1b in full plus the transition tests of 5.2.
- T-P2.2 (M) accounts adapters (T-P2a.2 and T-P2b.2). Owns `controller/src/adapters/{unix_chkpwd,authorized_keys_fs,ssh_keygen}.rs` and, after D-C2a merged, no file from wave B except via the orchestrator. `accounts_shadow.rs`, `ports.rs` and `adapters/mod.rs` belong to D-C2a, so T-P2.2 asks D-C2a's owner to append its trait items, or the orchestrator reassigns those three files to T-P2.2 after wave B. Default: reassign to T-P2.2 in this wave (D-C2a is merged).
- T-P2.4 (M) frontend logic (T-P2a.4 and T-P2b.4). Owns `frontend/src/users/**`, `frontend/src/state/**`, `frontend/src/transport/**`, `frontend/src/utils/**` and their unit tests (reassigned from D-F1 after it merged). Depends on D-C1 and D-F1.

### Wave D

- T-P2.5 (L) users service and wiring (T-P2a.5 and T-P2b.5). Owns `controller/src/services/{users,mod}.rs`, `controller/src/app/{router,policy,wire_map}.rs`, `controller/src/lib.rs`, `controller/src/utils/config.rs`, `config/controller.toml`, `config/README.md`, `controller/tests/{layering,no_secret_logging}.rs`. Depends on D-C2b, T-P2.1, T-P2.2, D-C1. Gates G2a-a and G2b-a.
- T-P2.6 (L) frontend screens and e2e (T-P2a.6 and T-P2b.6). Owns `frontend/src/menu/**`, `frontend/src/wifi/picker-view.js`, `frontend/index.html`, CSS, `frontend/tests/e2e/**`, `frontend/tests/menu-chart.test.js`. Depends on T-P2.4, D-C1. Gates G2a-b and G2b-b.

### Wave E

- D-D1 (M) docs. Covers the plan's T-P1.9, T-P2a.7 and T-P2b.7 in one task. Owns `docs/users.md`, `docs/terminal.md`, `docs/architecture.md`, `docs/development.md`, `docs/yocto-build-and-flash.md`, `docs/hardware.md`, `docs/dependency-surface.md`, `docs/research/yocto-build-decisions.md`, and a one-line pointer at the top of `docs/research/user-management-plan.md`. Mandatory contents beyond plan `:398`: password SSH for `kiosk` only, no key baked, key store for later keys, the unset screen and the two M presses (with "save it, no timeout, second press is final"), reboot regeneration, S3 adopt behaviour, faillock behaviour and the lockout denial of service, the single-factor root caveat, degraded mode has no remote access, factory reset recovery, terminal shells run as `kiosk` with `sudo` and `adm` (`handover.md:47`, correct plan `:156,355`), the removed codes, build no longer needs a public key, `ssh -L ... root@<device>` becomes `kiosk@<device>` in `docs/architecture.md`, `docs/yocto-build-and-flash.md` "accepts SSH as root" fix, the Chromium 147 and CDP note, the local websocket residual. Do not document `press_required`, `expired`, `not_pending`. `config/README.md` and `frontend/tests/README.md` stay with their owners and are done by wave C and D tasks. Depends on everything above. Gate G1e and G2a-c: every documented default equals `config/controller.toml` and the compiled default.

### Wave F (no ownership, read only)

Integration review by the orchestrator: full `cargo test` and `cargo clippy -- -D warnings`, `node --test frontend/tests/*.test.js` with `syntax-guard`, e2e or SKIP with reason, `python3 build/tools/test_check_image_accounts.py`, `build/test-build-image.sh`, `CONFIG_ONLY=1 build/build-image.sh`, `rg` greps of section 12, a multi-round review of the diffs for the 5.2 and 5.3 lists. Fixes go back to the file's wave owner.

### Wave G (owner or orchestrator only)

The single real build on devel from a clean `git archive HEAD` copy (check load first, `handover.md:43`), `bitbake -p` and flag passthrough proof (`handover.md:44`), `check_image_accounts.py` on the `.wic`, reconcile the duplicate `build-flags` write (`handover.md:46`), owner-approved flash, then device checklists G1d, G2a-d and G2b-d with the items of section 12. The device checks are U until then.

### Gates summary

- G1a (D-B1, D-B2, D-B3): config-only build and the build-script test pass; recipe parse and QA in wave G.
- G1b (D-C2a, D-C2b): `cargo test`, `cargo clippy -- -D warnings`, `layering`, `wire_golden`, `log_policy`, `no_secret_logging`, plus the I1 fault-injection test and the `ButtonPolicy` tests (inactive gate equals today's behaviour; active gate: no replay, no timer, no restart on a 10 s hold, release suppression, Enter suppression, autorepeat ignored, press-edge cases).
- G1c (D-K1): checker unit tests pass and fail on each bad fixture; on the real `.wic` in wave G.
- G1d: device only, section 12.
- G1e: docs equal config.
- G2a and G2b gates as in plan `:486-501`, run once on the merged result, with the 2b baked-key edits of section 9.
