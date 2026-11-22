# vendor/ — xterm.js (vendored)

`xterm.js` 5.3.0 (last stable npm release at vendoring time; 5.4.x was still beta), UMD/IIFE browser build, fetched from jsDelivr's npm mirror and committed here as static files (no CDN fetch at runtime, a convention the now-removed `controller/frontend/` dev-only demo UI — never shipped — used to follow too):

- `xterm.js` — `https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js` (the `lib/` build is the UMD one; `xterm.mjs` under a different path is the ES module build and is NOT used here — a `file://` page load has no Content-Type header, so `<script type="module">` fails to load at all, same constraint as this project's other non-module scripts). The trailing `//# sourceMappingURL=xterm.js.map` comment was stripped since the `.map` file isn't vendored and would otherwise 404 on every devtools open.
- `xterm.css` — `https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css`

Loaded in `index.html` before `app.js`:
```html
<link rel="stylesheet" href="vendor/xterm.css">
...
<script src="vendor/xterm.js"></script>
<script src="app.js"></script>
```

`app.js` creates one shared `Terminal({ disableStdin: true, ... })` instance, calls `term.open(document.getElementById('screen'))` once in `main()`, and feeds every WebSocket `output` message to it via `term.write(data)`. `disableStdin: true` keeps all keyboard handling in `app.js`'s existing `onKeyDown()` (registered on `#screen` with `{ capture: true }` so it always runs before xterm's own internal textarea listener could otherwise intercept/stop-propagate a keystroke) — xterm here is purely an ANSI/VT100 + curses-escape-code renderer, not an input widget. `daemon/src/main.rs`'s WebSocket protocol (`input`/`output`/`resize`/`control`/`clock_sync`/`session_list`/`exited`) needed zero changes for this.

- `xterm-addon-fit.js` — `https://cdn.jsdelivr.net/npm/xterm-addon-fit@0.8.0/lib/xterm-addon-fit.js` (the `lib/` build is UMD, exposing a global `FitAddon.FitAddon`; same file://-page / no-Content-Type reasoning as xterm.js itself applies — no ESM build here either, and the trailing `//# sourceMappingURL=xterm-addon-fit.js.map` comment was stripped for the same reason as xterm.js's own map comment). Used by `app.js` to compute real `cols`/`rows` from the `#screen` container's actual pixel size and the terminal's font metrics (`fitAddon.fit()` / `.proposeDimensions()`), instead of the old hardcoded `cols:80,rows:24` sent on every WS connect/resize/tab-switch — see `docs/terminal.md` or the git history around 2026-11 for why that hardcoding broke both glances' screen-width fill and (via a `SIGWINCH`-only-fires-on-change chain in `daemon/src/main.rs`'s `Session::resize()`) the bash prompt redraw on tab switch.

The device's kiosk runs Chromium 68 (owner-confirmed). That predates optional chaining, nullish coalescing, class fields, `Array.prototype.flat` and `Object.fromEntries`, so `app.js` and any vendored library must stay within an ES2018-level syntax ceiling, and native ES modules are blocked over `file://` (so vendored builds must be UMD/classic scripts). The currently vendored xterm.js and FitAddon builds are known to render on the device. Before any xterm.js upgrade, re-check the device version with `curl http://127.0.0.1:9223/json/version` (via an SSH tunnel to the device) and test the new build on hardware.
