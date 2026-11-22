# vendor: xterm.js (vendored)

Third-party browser builds committed here as static files, so the kiosk page loads them over `file://` with no network access at runtime:

| File | Source |
| --- | --- |
| `xterm.js` | `https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js` |
| `xterm.css` | `https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css` |
| `xterm-addon-fit.js` | `https://cdn.jsdelivr.net/npm/xterm-addon-fit@0.8.0/lib/xterm-addon-fit.js` |

All three are the UMD/classic builds from each package's `lib/` or `css/` directory, pinned to xterm 5.3.0 and xterm-addon-fit 0.8.0. The ES module builds are not used: a `file://` page load has no Content-Type header, so `<script type="module">` fails to load, the same constraint as the rest of the frontend's classic scripts. FitAddon exposes the global `FitAddon.FitAddon`.

## Provenance

The files were checked on 2026-11-22 against the pinned npm releases. `xterm.css` is identical. `xterm.js` and `xterm-addon-fit.js` differ from the releases only by the stripped trailing `//# sourceMappingURL=...` comment, removed because the `.map` files are not vendored and would 404 on every devtools open. To re-fetch or re-verify, download the three URLs above from the jsDelivr npm mirror at those pins and strip the same trailing comment from the two JS files. There is no fetch script in the repo.

## How the frontend uses them

`index.html` loads them as classic scripts before the app's own scripts:

```html
<link rel="stylesheet" href="vendor/xterm.css">
<script src="vendor/xterm.js"></script>
<script src="vendor/xterm-addon-fit.js"></script>
```

`src/terminal/terminal-view.js` creates one shared `Terminal` with `disableStdin: true` and loads FitAddon; `src/boot.js` opens it on `#screen`. xterm is only an output renderer: PTY output from the terminal agent WebSocket is written to it, and a tab switch resets it and replays that session's buffer. FitAddon computes the real `cols` and `rows` from the container size, and `fitAndResize()` sends them to the active session's PTY (when the channel goes live, on tab switch, on window resize and when the container is resized), instead of a fixed size.

Keyboard input never goes through xterm. `src/input/keyboard.js` binds a `keydown` handler on `#screen` in the capture phase (`TR.keyboard.bind(...)` in `src/boot.js`), so it runs before xterm's own textarea listener could stop the event. It maps keys to bytes, sends them with `agent.sendInput`, and swallows all keys while the menu is open.

## Browser constraint

The device kiosk runs Chromium 69. That predates optional chaining, nullish coalescing and class fields, so the frontend stays within an ES2018-level ceiling, enforced for its own sources by `tests/syntax-guard.test.js` (which excludes `vendor/`). The vendored builds are not scanned by it, so test any xterm.js or FitAddon upgrade on hardware against the kiosk's Chromium before committing it.
