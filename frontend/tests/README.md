# frontend tests

Unit tests run on node alone, with no npm packages:

```
node --test frontend/tests/*.test.js
```

Use the explicit glob. The bare directory form `node --test frontend/tests/` fails on Node 24 ("Cannot find module .../frontend/tests"). Files that are not `*.test.js` (`helpers/`, `e2e/`) are deliberately outside the glob. The frontend sources are classic scripts that also load as CommonJS under node, so the tests `require` them directly; `helpers/` holds shared fakes (socket, timers, statechart harness) and a small TOML-subset reader.

## Syntax guard

`syntax-guard.test.js` fails when a shipped file under `frontend/` uses syntax or APIs newer than a conservative baseline from the days when the kiosk was believed to run Chromium 68 or 69 (ES2018-level ceiling, no newer CSS). The kiosk actually runs Chromium 147 (verified on the device on 2026-11-22), so the guard is stricter than the browser requires; see the [Chromium 147 frontend audit](../../docs/research/chromium-147-frontend-audit.md). It runs a token scan always, a real acorn parse at ecmaVersion 2019 when acorn can be resolved offline (`ACORN_PATH`, `require('acorn')` or a `node_modules` found by walking up; acorn is not a dependency of this repo), `node --check` on every source, and a CSS property scan. `vendor/` and `tests/` are excluded.

## First-boot password screens

`users.test.js` covers the first-boot reveal (T-P1.8): command builders and decoders against the golden `user_bootstrap*` and `user_error` fixtures, the reducer slice (status only, never the password), the log redaction of `password`, `last4` and `current_password`, the screens as data (reveal, countdown, confirm picker, `bad_ack`, expiry, disconnect and leave wiping the secret) and the secret view's DOM. The e2e script has two matching scenarios (reveal and confirm; wipe on back, close, disconnect and reload). One check in the e2e run, `buttons 1..4 map to sessions 0..3`, is timing dependent and also fails intermittently on a checkout without these changes.

## Wire fixtures

Wire messages come only from the shared golden fixtures under `config/protocol/controller` and `config/protocol/terminal` (see `config/protocol/README.md`). There are no local copies, and a missing fixture fails the run. Assertions pass explicit overrides for the fields they check.

## Headless e2e regression

```
node frontend/tests/e2e/menu-regression.mjs
```

This drives the real `index.html` over `file://` at 800x480 with mocked WebSockets (`e2e/mock-server.mjs`, `e2e/mock-websocket.js`). It needs a playwright library and a Chromium on the host; nothing is added to the repo. The library is looked up through `$PLAYWRIGHT_MODULE`, normal node resolution, `npm root -g` and the mise playwright-cli install; the browser through `$CHROME_PATH`, `$PLAYWRIGHT_BROWSERS_PATH` or `~/.cache/ms-playwright`, then a system chromium. When either is missing the script prints a `SKIP` line and exits 0. Set `E2E_VERBOSE=1` to list passing checks. Being a `.mjs` under `e2e/`, it is never picked up by the `*.test.js` glob.
