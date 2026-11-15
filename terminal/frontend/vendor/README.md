# vendor/ (empty — known follow-up)

This directory is where a real vendored copy of `xterm.js` (the UMD/IIFE
browser build, e.g. `xterm.js` + `xterm.css` from the `xterm` npm package,
plus `xterm-addon-fit` if cols/rows auto-sizing is wanted later) should be
copied in a future session that has normal internet/npm access.

It is empty in this scaffold because this sandbox had no outbound network
path to fetch the package, and the project's own convention (`control-surface/frontend/`,
`dashboard/frontend/`) is "no CDN fetch assumed available on-device" — a
file dropped here must be committed to the repo and pushed with the rest
of the static assets, never fetched at runtime.

Until then, `terminal/frontend/app.js` uses a minimal `<pre>`-based line
buffer renderer that speaks the exact same WebSocket JSON protocol
documented in `docs/terminal.md` (`input`/`output`/`resize`/`control`
messages against `terminal/agent/`, the Rust `terminal-agent` binary).
Swapping in real
`xterm.js` later is a frontend-only change:

1. Drop `xterm.js` (non-module build) and `xterm.css` into this directory.
2. In `index.html`, add `<link rel="stylesheet" href="vendor/xterm.css">`
   and `<script src="vendor/xterm.js"></script>` before `app.js`.
3. In `app.js`, replace the `renderScreen()`/`appendOutput()` functions
   with a real `Terminal` instance (`new Terminal()`, `term.open(el)`,
   `term.write(data)`, `term.onData(cb)`), keeping every WebSocket message
   shape identical — the agent side needs no changes at all.

Confirm Chromium 69 compatibility before shipping: xterm.js's UMD build
targets ES2017+ in some versions; pin to a version confirmed to parse under
Chromium 69 (see `docs/hardware.md`), the same silent-parse-failure risk
`docs/dashboard.md` already hit once with ES2020 syntax.
