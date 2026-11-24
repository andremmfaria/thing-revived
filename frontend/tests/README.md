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

`accounts-menu.test.js` drives the real chart, screens and reducers (`helpers/chart-harness.js`) through the Users section of the menu: the unlock gate and what is wiped after sending, the list, the user page and its guard texts, add user (generated or typed password), the one-time password screen (`user_secret`) and every way it is cleared (Done, back, lock, expiry), idle lock and `not_unlocked`, session loss, group toggles with the sudo confirm, key add and remove, per-screen input limits, and the hold-to-confirm mechanics (`hold: true` rows fire from the `enter-hold` timer while the knob push is still held; an early release, an ESC tap or a stale timer send nothing). The e2e script has five matching scenarios (unlock, add user with the one-time password, hold-to-confirm, groups and keys, first-boot overlay still blocking). The old first-boot reveal screens (`menu/screens/users.js`) no longer exist; first boot is `firstboot/first-boot.js` only.

## USB mode menu

`usb-role.test.js` drives the real chart, screens and reducers through Settings, USB mode: the row (listed without a Wi-Fi adapter), the screen text for the running and the saved role, the ADB dev build note, `usb_role_set` as the only change command (never a live switch), the hold-to-confirm Reboot now row (tap and ESC tap only hint, the held push sends `usb_role_reboot`, a stale timer sends nothing), error and announcement toasts, and the frame builders against the golden fixtures. The e2e script has one matching scenario that also checks the screen fits 800x480 without scrolling or clipped labels.

## Debug access

`debug-access.test.js` drives the real chart, screens and reducers through Settings, Debug access: the Users unlock gate (locked shows the same locked notice as Users, not the toggle rows), the reducer slice and the badge-activity and host-block selectors, the hold-to-confirm rows for each surface (a tap or an ESC tap only hints, the held push sends `debug_set`, a stale timer sends nothing), the ADB row disabled with a note when `debug_state.blocked` is `usb_role_host`, the persistent error line (a note, not a toast, cleared on leaving and on re-entering), the always-non-optimistic state (only a fresh `debug_state` flips the shown ON/OFF), and the frame builders against the golden fixtures. The e2e script has one matching scenario that also checks the unlock gate, the DEBUG badge on both the menu and the terminal view, the badge hidden during the first-boot overlay, and 800x480 geometry (no clipped row labels, the badge inside the status bar, the terminal fit still ending above the status bar, no layout shift when the badge toggles).

Wire naming: the shipped family is `debug_get`/`debug_set` and `debug_state`/`debug_error` with a `surface` field (`adb` or `uart`), not the research plan's `debug_access_*` names and `kind` field; see `config/protocol/controller/README.md`. `debug_set` always carries `confirm` equal to `surface`, sent only by the held gesture, never typed.

## Wire fixtures

Wire messages come only from the shared golden fixtures under `config/protocol/controller` and `config/protocol/terminal` (see `config/protocol/README.md`). There are no local copies, and a missing fixture fails the run. Assertions pass explicit overrides for the fields they check.

## Headless e2e regression

```
node frontend/tests/e2e/menu-regression.mjs
```

This drives the real `index.html` over `file://` at 800x480 with mocked WebSockets (`e2e/mock-server.mjs`, `e2e/mock-websocket.js`). It needs a playwright library and a Chromium on the host; nothing is added to the repo. The library is looked up through `$PLAYWRIGHT_MODULE`, normal node resolution, `npm root -g` and the mise playwright-cli install; the browser through `$CHROME_PATH`, `$PLAYWRIGHT_BROWSERS_PATH` or `~/.cache/ms-playwright`, then a system chromium. When either is missing the script prints a `SKIP` line and exits 0. Set `E2E_VERBOSE=1` to list passing checks. Being a `.mjs` under `e2e/`, it is never picked up by the `*.test.js` glob.
