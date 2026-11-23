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

## Wire fixtures

Wire messages come only from the shared golden fixtures under `config/protocol/controller` and `config/protocol/terminal` (see `config/protocol/README.md`). There are no local copies, and a missing fixture fails the run. Assertions pass explicit overrides for the fields they check.

## Headless e2e regression

```
node frontend/tests/e2e/menu-regression.mjs
```

This drives the real `index.html` over `file://` at 800x480 with mocked WebSockets (`e2e/mock-server.mjs`, `e2e/mock-websocket.js`). It needs a playwright library and a Chromium on the host; nothing is added to the repo. The library is looked up through `$PLAYWRIGHT_MODULE`, normal node resolution, `npm root -g` and the mise playwright-cli install; the browser through `$CHROME_PATH`, `$PLAYWRIGHT_BROWSERS_PATH` or `~/.cache/ms-playwright`, then a system chromium. When either is missing the script prints a `SKIP` line and exits 0. Set `E2E_VERBOSE=1` to list passing checks. Being a `.mjs` under `e2e/`, it is never picked up by the `*.test.js` glob.
