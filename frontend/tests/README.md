# frontend tests

Unit tests (node only, no npm packages):

```
node --test frontend/tests/*.test.js
```

Use the explicit glob. The bare directory form `node --test frontend/tests/` fails on Node 24 ("Cannot find module .../frontend/tests"). Files that are not `*.test.js` (helpers, `e2e/`) are deliberately outside the glob.

Headless regression run (drives the real `index.html` over `file://` at 800x480 with mocked WebSockets):

```
node frontend/tests/e2e/menu-regression.mjs
```

It needs a playwright library and a Chromium on the host; nothing is added to the repo. The library is looked up through `$PLAYWRIGHT_MODULE`, normal node resolution, `npm root -g` and the mise playwright-cli install; the browser through `$CHROME_PATH`, `$PLAYWRIGHT_BROWSERS_PATH` or `~/.cache/ms-playwright`, then a system chromium. When either is missing the script prints a `SKIP` line and exits 0. Set `E2E_VERBOSE=1` to list passing checks.

Wire messages come only from the shared fixtures under `config/protocol/controller` and `config/protocol/terminal` (no local copies; a missing fixture fails the run); every assertion passes explicit overrides for the fields it checks. The e2e script is a `.mjs` under `e2e/`, so the CI glob `node --test frontend/tests/*.test.js` never runs it.
