# frontend tests

Unit tests run on node alone, with no npm packages:

```
node --test frontend/tests/*.test.js
```

Use the explicit glob. The bare directory form `node --test frontend/tests/` fails on Node 24 ("Cannot find module .../frontend/tests"). Files that are not `*.test.js` (`helpers/`, `e2e/`) are deliberately outside the glob. The frontend sources are classic scripts that also load as CommonJS under node, so the tests `require` them directly; `helpers/` holds shared fakes (socket, timers, statechart harness) and a small TOML-subset reader.

## Syntax guard

`syntax-guard.test.js` fails when a shipped file under `frontend/` uses syntax or APIs newer than a conservative baseline from the days when the kiosk was believed to run Chromium 68 or 69 (ES2018-level ceiling, no newer CSS). The kiosk actually runs Chromium 147 (verified on the device on 2026-11-22), so the guard is stricter than the browser requires; see the [Chromium 147 frontend audit](../../docs/research/chromium-147-frontend-audit.md). It runs a token scan always, a real acorn parse at ecmaVersion 2019 when acorn can be resolved offline (`ACORN_PATH`, `require('acorn')` or a `node_modules` found by walking up; acorn is not a dependency of this repo), `node --check` on every source, and a CSS property scan. `vendor/` and `tests/` are excluded.

## First-boot password screens

`users.test.js` covers the first-boot wire pieces (T-P1.8 as reworked by D-F1): the `user_bootstrap_get` builder, decoders against the golden `user_bootstrap*` and `user_error` fixtures (one per phase), the reducer slice (phase only, never the password) and log redaction. `first-boot.test.js` covers the overlay: the phase model, the view, the document-level key swallow, the chart input gate (ESC, buttons 1-4, Enter, knob, taps, text), the focus gate and the `sendInput` sink wiring. The e2e script has four matching scenarios (prompt to done, every input path dead while up, wipe on close and reload, storage note). One check in the e2e run, `buttons 1..4 map to sessions 0..3`, is timing dependent and also fails intermittently on a checkout without these changes.

## Account menu (users, groups, SSH keys)

`accounts-menu.test.js` drives the real chart, screens and reducers (`helpers/chart-harness.js`) through the Users section of the menu (reached through Device settings after the rework, see the next section): the unlock gate and what is wiped after sending, the list, the user page and its guard texts, add user (generated or typed password), the one-time password screen (`user_secret`) and every way it is cleared (Done, back, lock, expiry), the fixed 30 minute expiry and `not_unlocked` (the old idle lock is gone), session loss, group toggles with the sudo confirm, key add and remove, per-screen input limits, and the hold-to-confirm mechanics (`hold: true` rows fire from the `enter-hold` timer while the knob push is still held; an early release, an ESC tap or a stale timer send nothing). The e2e script has five matching scenarios for the old Settings, Users entry, which are being rewritten (unlock, add user with the one-time password, hold-to-confirm, groups and keys, first-boot overlay still blocking). The old first-boot reveal screens (`menu/screens/users.js`) no longer exist; first boot is `firstboot/first-boot.js` only.

## Device settings menu

`device-settings.test.js` drives the real chart, screens and reducers through Device settings and its USB mode screen. It covers: the locked state (every row after Unlock is disabled, so the knob highlights it but a push or tap does not select it), unlocking enabling the rows, the Unlock row turning into Lock now (which sends `user_lock_session`), the toggle rows sending the right `device_settings_set` frame for Mode, SSH, ADB and UART, Mode host greying the ADB row with its note, the busy state disabling rows, expiry of the 30 minute session returning the rows to disabled, and the hold-to-confirm Reboot device row (tap and ESC tap only hint, the held push sends `device_reboot`, a stale timer sends nothing). `device-settings-logic.test.js` covers the reducer slice and selectors (live versus persisted values, the drift note, the DEBUG badge from live adb or uart) and `menu-view-disabled.test.js` covers the disabled row rendering. `accounts-menu.test.js` is being reworked for the same change: unlocking from Device settings returns to the Device settings root, there is no idle timer, leaving Users does not lock, and `session_locked` or `not_unlocked` pop to the Device settings root. The frame builders and decoders are checked against the golden `device_settings*`, `device_reboot` and `device_rebooting` fixtures and the `user_session` fixture with `expires_in_secs` 1800.

## Device settings e2e scenarios

The e2e scenarios for the old USB mode, Debug access and Users screens are being rewritten and until that lands they do not match the shipped screens. The planned Device settings scenarios (`docs/research/device-settings-plan.md` section 8.2) are: locked and greyed rows, unlock returning to Device settings, Lock now, expiry popping to the Device settings root, five rows fitting 800x480 without scrolling, the USB mode screen (four rows plus one note plus hint) fitting with the measured row height, the DEBUG badge on both the menu and the terminal view and hidden during the first-boot overlay, and Host greying ADB. They are planned, not yet run against the new screens.

The wire family is `device_settings_get`, `device_settings_set` (`setting`, `value`) and `device_reboot` with the events `device_settings`, `device_settings_error` and `device_rebooting`. There is no `confirm` field: hold-to-confirm is a frontend concern, and the controller's real gate is the unlock. See `config/protocol/controller/README.md`. The `debug_*` and `usb_role_*` families are replaced by it.

## Wire fixtures

Wire messages come only from the shared golden fixtures under `config/protocol/controller` and `config/protocol/terminal` (see `config/protocol/README.md`). There are no local copies, and a missing fixture fails the run. Assertions pass explicit overrides for the fields they check.

## Headless e2e regression

```
node frontend/tests/e2e/menu-regression.mjs
```

This drives the real `index.html` over `file://` at 800x480 with mocked WebSockets (`e2e/mock-server.mjs`, `e2e/mock-websocket.js`). It needs a playwright library and a Chromium on the host; nothing is added to the repo. The library is looked up through `$PLAYWRIGHT_MODULE`, normal node resolution, `npm root -g` and the mise playwright-cli install; the browser through `$CHROME_PATH`, `$PLAYWRIGHT_BROWSERS_PATH` or `~/.cache/ms-playwright`, then a system chromium. When either is missing the script prints a `SKIP` line and exits 0. Set `E2E_VERBOSE=1` to list passing checks. Being a `.mjs` under `e2e/`, it is never picked up by the `*.test.js` glob.
