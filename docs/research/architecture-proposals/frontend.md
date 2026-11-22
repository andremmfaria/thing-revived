# frontend/ architecture proposals

Status: research proposal, no code changed. Scope: `frontend/` only (HTML/CSS/vanilla JS kiosk UI). Sibling analyses cover `controller/` and `daemon/`; section 6 lists what must be agreed with them.

## 0. Summary and premise corrections

The frontend is one 1573-line classic script (`app.js`) plus one 161-line pure helper (`wifi-logic.js`). Roughly 56% of `app.js` (lines 576 to 1456) is the settings menu, and almost all of the pain comes from there: menu state, navigation, input semantics, wire-message handling and DOM rendering are interleaved through one global `state` object. The terminal half (tabs, xterm, two WebSockets, clock) is small and fine. The real problem is not rendering or lack of a framework. It is that control flow (knob/button semantics, ESC tap/hold, the 400 ms double-delivery guard, async arrivals that pop screens) lives in mutable flags and closures that can only be exercised in a browser. Ten node tests cover about 10% of the code, and CI does not even run them.

Recommendation in one line: keep the no-build, classic-script delivery for now, but restructure into a pure core (one reducer-style interaction state machine plus injected effects, with the existing declarative screen tree turned into data) and thin DOM views, with a shared `protocol/` JSON Schema plus fixtures checked by tests on both sides. Treat esbuild as a documented escape hatch with explicit triggers, not as step one.

Premise corrections found while researching:

- `docs/development.md` says no smart-home client is part of this project. The real backlog items that affect UI architecture (from `docs/coming-soon.md`) are: font size and color theme in settings, an on-screen keyboard fallback, per-tab activity indicator, scrollback search addon, copy/paste via touch plus menu action, and an idle-screen clock/status overlay. Any multi-widget dashboard-style view is a hypothetical future driver, not a committed one, and I say so where it matters.
- The task brief says ES modules over `file://` need checking. I checked it empirically (section 1.4). Result: blocked by default.
- Stale facts in the repo that affect decisions: `frontend/vendor/README.md` said Chromium 147 was confirmed live on the device, and `docs/research/hardware-verification.md` said Chromium 69. **Correction (2026-11-22): the owner confirms the device kiosk runs Chromium 68.** 147 is the upstream yocto-superbird README claim for a different build and does not apply to this device; 69 was an off-by-one in earlier research. The `?.` and `??` syntax ban at `app.js:1-8` ("for consistency with the sibling controller/ frontend", a sibling that no longer exists, `README.md:34`) must therefore stay: Chromium 68 predates both. The conclusions below were rewritten for a Chromium 68 target, and section 1.6 is a compatibility cheat sheet.

## 1. Current state, measured

### 1.1 Files and install path

| File | Lines | Role |
|---|---|---|
| `frontend/app.js` | 1573 (62 KB) | everything |
| `frontend/wifi-logic.js` | 161 (5.4 KB) | pure Wi-Fi helpers, UMD-style dual load |
| `frontend/index.html` | 74 | static shell, 4 classic `<script>` tags (`index.html:66-69`) |
| `frontend/style.css` | 380 (7.6 KB) | all styling |
| `frontend/tests/wifi-logic.test.js` | 94 | 10 node tests, all pass (`node --test frontend/tests/wifi-logic.test.js`) |
| `frontend/vendor/` | xterm.js 283 KB, xterm.css 5 KB, fit addon 1.5 KB | UMD builds, committed |

Install path: `build/build-image.sh` step 4 does `rsync -a --delete frontend/ -> recipe files/`, then the Yocto recipe `superbird-kiosk-default-webapp.bb` lists each file individually in `SRC_URI` (lines 5-12) and `do_install` (lines 19-26). Consequences: adding any new file requires editing the recipe in two places and `index.html`, and the whole `tests/` directory is rsynced but never installed. The device serves via `file://` from a read-only rootfs (stock launch line per `docs/research/architecture-decision.md:25`: `chrome --kiosk --no-sandbox --in-process-gpu --app=file:///...`). The actual flags in the yocto-superbird upstream layer are not in this repo and I could not verify them.

CI (`.github/workflows/ci.yml:55-66`) runs only `node --check frontend/app.js`. The node tests are not run in CI. Also, `node --test frontend/tests/` (the invocation in the test file header) fails on Node 24 with "Cannot find module .../frontend/tests"; only the explicit file form works.

### 1.2 Sections of app.js (lines, share)

| Section | Lines | Count | Share |
|---|---|---|---|
| Header, constants, `WL` alias (`app.js:1-60`) | 1-60 | 60 | 3.8% |
| Global `state` (`62-109`) | 62-109 | 48 | 3.1% |
| xterm + FitAddon setup, `fitAndResize` (`111-152`) | 111-152 | 42 | 2.7% |
| DOM helpers, tabs, output buffering, screen replay, `sendAgent/sendBridge` (`154-220`) | 154-220 | 67 | 4.3% |
| `connectAgent` (agent WS, 8766) (`222-266`) | 222-266 | 45 | 2.9% |
| `connectBridge` (controller WS, 8765) (`268-316`) | 268-316 | 49 | 3.1% |
| `handleButton` incl. ESC tap/hold timer (`318-384`) | 318-384 | 67 | 4.3% |
| `handleSettingsEvent`, wire message to state (`386-526`) | 386-526 | 141 | 9.0% |
| knob scroll + `switchToSession` (`528-558`) | 528-558 | 31 | 2.0% |
| clock (`560-574`) | 560-574 | 15 | 1.0% |
| menu doc comment + `rootEntries` (`576-621`) | 576-621 | 46 | 2.9% |
| `menuScreens()` declarative screen tree (`623-997`) | 623-997 | 375 | 23.8% |
| of which Wi-Fi screens | 633-797 | 165 | 10.5% |
| of which Bluetooth screens | 798-927 | 130 | 8.3% |
| of which terminal/font screens | 928-959 | 32 | 2.0% |
| of which timezone screens | 960-995 | 36 | 2.3% |
| timezone grouping helpers | 999-1025 | 27 | 1.7% |
| menu stack navigation (`currentScreen`, push/pop/popTo) | 1027-1076 | 50 | 3.2% |
| Wi-Fi screen helpers, text edit, knob picker render | 1078-1186 | 109 | 6.9% |
| open/close/toggle menu, ESC timer clear | 1188-1220 | 33 | 2.1% |
| `menuKnob`, `menuSelect`, `menuBack` | 1222-1318 | 97 | 6.2% |
| toast | 1320-1328 | 9 | 0.6% |
| `renderMenu` | 1330-1417 | 88 | 5.6% |
| `menuHintText` (if-chain on screen ids) | 1419-1456 | 38 | 2.4% |
| keyboard to PTY bytes, `focusScreen` | 1458-1523 | 66 | 4.2% |
| `main()` wiring | 1525-1573 | 49 | 3.1% |

Settings menu in total (`576-1456` plus the settings half of `386-526`): about 880 lines. Counts: 168 top-level functions/consts, 104 references to `state.menu`, 35 direct DOM lookups/creations, 30 `sendBridge(` call sites emitting 22 distinct command types (`brightness_get/set`, `bt_*` x10, `timezone_*` x3, `wifi_*` x7).

### 1.3 State, events, rendering, testability

Global state: a single module-level `const state` (`app.js:62`) holding terminal data (sessions, per-session chunk buffers, active index, dead sessions, clock offset), both sockets and ready flags, and `state.menu` with navigation stack, per-screen highlight map, `adjusting`, ESC timer ids, bt/brightness/timezone/fontSize/wifi sub-trees and toast. Plus three more globals: `term`, `fitAddon`, `WL`. Domain data (what the daemon told us), view-model (what is highlighted), transient input state (`escPressedAt`, `escHoldTimer`, `editFinishedAt`) and live handles (sockets, timers) all live in one mutable bag, and everything mutates it directly. One undeclared field is smuggled in: `state.menu._toastTimer` (`app.js:1323`).

Event flow today:

```
controller WS 8765 (onmessage 297-315)
  knob   -> menu open ? menuKnob : term.scrollLines
  button -> handleButton -> toggleMenu | ESC tap/hold timer | menuSelect/menuBack | switchToSession
  other  -> handleSettingsEvent (mutate state.menu, sometimes openMenu/pushScreen/popScreen) -> renderMenu
terminal-agent WS 8766 (onmessage 238-265)
  session_list -> renderTabs + renderScreen ; output -> appendOutput -> term.write ; exited -> synthetic output ; clock_sync -> renderClock
DOM
  keydown on #screen (capture) -> onKeyDown -> sendAgent input (swallowed entirely when menu open, 1488)
  click/touchstart on .term-shell -> focusScreen ; row click -> menuSelect ; wifi <input> input/keydown/blur -> state.menu.wifi.*
timers
  ESC hold 450 ms ; toast 4 s ; reconnect 2 s fixed ; clock 1 s interval
```

Rendering approach: fully imperative. `renderTabs` and `renderMenu` clear with `innerHTML = ''` and rebuild (`app.js:164`, `1358`); status dots are mutated through `setStatus` (`154`). `renderMenu` is called from about 30 places (every state mutation remembers to call it, or it is skipped, e.g. `default: return` at `523`). The Wi-Fi password `<input>` had to be placed outside `#menu-list` in `index.html` so the full rebuild does not steal its focus, a direct symptom of rebuild-everything rendering. `menuScreens()` (the whole screen tree with closures) is rebuilt on every call, and `currentScreen()` calls it, so a single render builds it several times (`1033`, `1053`, `1062`). It is cheap at this size, but it shows the tree is code, not data.

Testability: `wifi-logic.js` is the only pure module. It is an IIFE taking `root`, ending with `if (typeof module !== 'undefined' && module.exports) module.exports = api; else root.WifiLogic = api;` (`wifi-logic.js:7`, end of file). Browser: classic `<script>` sets `window.WifiLogic`, `app.js:60` aliases it. Node: `require('../wifi-logic.js')` (`tests/wifi-logic.test.js:4`). That pattern works and is the template for the recommendation. Everything else is DOM-coupled or socket-coupled: screen definitions call `sendBridge`, `term.options`, `document.getElementById` inside closures (`715`, `938`, `953`); `handleSettingsEvent` calls navigation and toasts that call the DOM; `menuSelect` reads `Date.now()` (`1266`). `docs/wifi.md:145` admits the end-to-end menu check was a one-off headless run "not committed", so the interaction logic has no regression net.

### 1.4 ES modules over file:// (verified, not assumed)

Test: temp copy under `~/.hermes/cache/scratch/esm`, Chromium build 156.0.8078.4 (the playwright-installed binary; **correction 2026-11-22: the device runs Chromium 68, not 147, so this binary is far newer than the device. The CORS block on `file://` modules is long-standing behavior, but the "works with the flag" row is unverified on 68**), page at `file:///.../index.html` with `<script type="module" src="./main.js">`, an inline module importing `./dep.js`, and a dynamic `import('./lazy.js')`.

| Launch flags | Classic script | Module `src=` + static import | Dynamic import |
|---|---|---|---|
| default | runs | blocked: "Access to script at 'file:///.../main.js' from origin 'null' has been blocked by CORS policy" | blocked |
| `--in-process-gpu` (kiosk flag) | runs | blocked | blocked |
| `--allow-file-access-from-files` | runs | works (`modok=42`, `inl=41`, `lazy='lazy'`) | works |

So `vendor/README.md`'s claim stands (module scripts do not work under `file://`), though the stated reason ("no Content-Type header") is imprecise: the failure is the CORS check on an opaque `null` origin. Implications: native ES modules (supported from Chromium 61, so the engine is not the limit) require either adding `--allow-file-access-from-files` to the kiosk launch line, which lives in the upstream yocto-superbird layer and not in this repo's overlay (changing it means owning a bbappend for the weston/kiosk launcher, and the flag weakens the sandbox for every page the kiosk can load, mitigated only because the kiosk loads one local page), or serving the app over `http://localhost` from one of the daemons (new attack surface and a new moving part), or bundling to a single classic IIFE. A tiny inline `<script type="module">` with no imports works but is useless for splitting. Note (2026-11-22): both agents were bound to `0.0.0.0` when this was written; they are being changed to loopback-only (`127.0.0.1:8765` and `127.0.0.1:8766`), see the Security model in `docs/architecture.md`.

I also verified that esbuild is cheap to obtain and fast: `npm i esbuild` gave a 12 MB `node_modules` (one native binary, `@esbuild/linux-x64`) and bundled+minified the current `app.js` to 25 KB in 13 ms. Node 24.22 here strips TypeScript types natively (`node a.ts` ran). CI pins Node 20, which does not.

### 1.5 Smells and real pain points (with evidence)

1. Control flow encoded as flags and timers, not states. ESC tap/hold uses `escPressedAt` + `escHoldTimer` + defensive clearing in three places (`app.js:336-358`, `1199-1213`). The double-delivery workaround for knob-push (bridge `button` plus uinput-replayed Enter) is a magic 400 ms timestamp check (`1266`) that also lives in `state.menu.wifi.editFinishedAt`. These are the highest-risk lines in the file and the least testable.
2. Wi-Fi sub-state leaks into generic navigation. `menuKnob`, `menuSelect`, `menuBack`, `menuHintText` and `renderMenu` all special-case `state.menu.wifi.editing` or screen ids (`1224`, `1259`, `1308`, `1434-1445`, `1352`). Adding another text-entry screen means touching all five.
3. Screens are not self-describing. The hint bar is an if-chain on screen ids (`1424-1456`) separate from the screen definitions; the title/hint/entries/handlers of one screen are in three places.
4. Entries close over side effects. Each entry calls `sendBridge` or `term.options` directly (`609`, `938`), so "what does selecting this row do" cannot be asserted without stubbing sockets and xterm.
5. Wire handling mixes state update, navigation and UI. `handleSettingsEvent` (141 lines) opens the menu and pushes screens on pairing requests (`435-442`), pops screens when a list changes (`420-431`, `512-515`), and shows toasts, in one switch.
6. No transport abstraction. `connectAgent` and `connectBridge` are near duplicates (`222-266`, `268-316`); fixed 2 s retry, no backoff, no queue; `sendBridge`/`sendAgent` silently drop messages when not ready (`210-220`), so a `wifi_connect` sent during a reconnect vanishes with no feedback. JSON parse errors and unknown message types are swallowed silently (`243`, `302`, `523`), which hides protocol drift.
7. Footprint: terminal replay buffers cap at 2000 chunks per session (`app.js:178`), not bytes, and are replayed with `term.reset()` plus N `term.write()` on every tab switch (`196-200`). A chatty curses app (glances is session 0) can make chunks large; 4 sessions x unbounded bytes on a 512 MB device sharing RAM with Chromium. xterm's own scrollback duplicates this data. Worth capping by bytes regardless of architecture.
8. Tooling gaps: tests not in CI, test directory invocation broken on Node 24, recipe lists files by hand, stale comments (`index.html:68` mentions `terminal/frontend/vendor/`, `app.js:233` mentions `ui/`).

What keeps arriving, by evidence in the docs: more menu screens (Bluetooth, brightness, timezone, Wi-Fi each added in the last few commits, `git log` shows Wi-Fi alone added about 400 lines; font size/theme is next in `docs/coming-soon.md`), more input mappings (the ESC and KEY_M roles already swapped once, `settings-menu-feasibility.md` section 3; section 12 revised ESC semantics), an on-screen keyboard fallback and idle-screen clock overlay (both in `docs/coming-soon.md`; the second needs an idle/activity concept the app does not have), per-tab activity indicators (needs per-session state beyond the active tab), and, if it ever happens, dashboards. Each of those hits the same three seams: input semantics, screen definitions, wire handling.

### 1.6 Chromium 68 compatibility cheat sheet (added 2026-11-22)

The device kiosk runs Chromium 68 (owner-confirmed, 2018 vintage, V8 6.8). Version numbers are the first Chromium release with support, from memory of caniuse and chromestatus. Entries marked "verify" should be checked on the device (`curl http://127.0.0.1:9223/json/version` through an SSH tunnel, then a quick evaluation in the remote-debugging console) before relying on them.

| Feature | First Chromium | Safe on 68 |
|---|---|---|
| `async`/`await` | 55 | yes |
| Object spread/rest (`{...a}`) | 60 | yes |
| Async iteration, `for await` | 63 | yes |
| `Promise.prototype.finally` | 63 | yes |
| Optional catch binding (`catch {}`) | 66 | yes |
| Regex `s` flag, lookbehind | 62 | yes (verify) |
| Regex named groups | 64 | yes (verify) |
| Classes, arrow functions, `let`/`const`, template literals | 49 or earlier | yes |
| Class public fields (`x = 1;` in a class body) | 72 | NO |
| Private class fields (`#x`) | 74 | NO |
| Optional chaining `?.` | 80 | NO |
| Nullish coalescing `??` | 80 | NO |
| `Array.prototype.flat`/`flatMap` | 69 | NO |
| `String.prototype.matchAll` | 73 | NO |
| `Object.fromEntries` | 73 | NO |
| `globalThis` | 71 | NO |
| `Promise.allSettled` | 76 | NO |
| `String.prototype.replaceAll` | 85 | NO |
| `Array.prototype.at` | 92 | NO |
| `structuredClone` | 98 | NO |
| `BigInt` | 67 | yes (verify) |
| `fetch`, `Promise`, `Map`/`Set`, `URL`, `AbortController` | 66 or earlier | yes |
| `ResizeObserver` | 64 | yes |
| `IntersectionObserver` | 51 | yes |
| Native ES modules (`<script type="module">`) | 61 | engine yes, blocked over `file://` by CORS (section 1.4) |
| Dynamic `import()` | 63 | engine yes, same `file://` block |
| Custom elements v1 | 54 | yes |
| Shadow DOM v1 | 53 | yes |
| CSS grid | 57 | yes |
| CSS custom properties (`var(--x)`) | 49 | yes |
| Flexbox `gap` | 84 | NO (use margins) |
| Grid `gap` | 66 | yes (verify, `grid-gap` since 57) |
| `position: sticky` | 56 | yes |
| `aspect-ratio` | 88 | NO |
| `:is()`, `:where()` | 88 | NO |
| `inset` shorthand | 87 | NO |
| `backdrop-filter` | 76 | NO |
| `clamp()`, `min()`, `max()` | 79 | NO |
| `:focus-visible` | 86 | NO |
| `prefers-color-scheme` | 76 | NO |
| `@supports` | 28 | yes |

Consequences for the options above:

- **Classic scripts, no build (the recommendation)**: stay inside the "yes" rows. An ES2018 ceiling is the right mental model: `async`/`await`, object spread, `finally` and optional catch binding are fine. `?.`, `??`, class fields, `flat`, `matchAll` and `fromEntries` are not.
- **Native ES modules**: the engine supports them (61+), but over `file://` they are blocked by the CORS check unless the kiosk gets `--allow-file-access-from-files`, which lives in the upstream layer. That makes the no-build recommendation (classic scripts with the dual-load wrapper) even clearer, and a bundler step the only way to use modern syntax.
- **esbuild**: use `--target=chrome68 --format=iife`. It lowers syntax (`?.`, `??`, class fields) but adds no runtime polyfills, so the API rows marked NO still fail at runtime.
- **Web Components**: custom elements v1 and shadow DOM v1 exist (54 and 53), so they are viable. Risks: no constructable stylesheets (`adoptedStyleSheets`, 73), no class fields in element definitions (72), no form-associated custom elements (verify), and xterm.js sizing inside a shadow root needs care. Verify on the device before committing to them.
- **Preact/htm UMD (option 4b)**: current Preact UMD builds target old engines, so they should run, but pin and test the exact version on the device (verify). Anything ESM-only (XState v5, lit without a bundler) is out.
- **xterm.js 5.3.0**: the vendored build was verified rendering on the device (`docs/research/terminal-hardware-verification.md`, which recorded the browser as Chromium 69; it is 68), so it is known to work there. Re-check on the device before upgrading xterm.js, since newer releases may raise their syntax floor.
- **CSS**: grid (57) and custom properties (49) are safe. Flexbox `gap` (84), `clamp()`, `aspect-ratio`, `inset` and `:is()` are not.

## 2. Shared design vocabulary (used by all options)

Whatever pattern is chosen, three abstractions are needed and are the same across options, so they are defined once.

Input abstraction. Normalize everything to semantic events before any logic sees it:

```
{ kind: 'knob',   delta: 1 | -1 | n }                       from bridge `knob`
{ kind: 'button', code, phase: 'down' | 'up' | 'repeat' }    from bridge `button` (value 1/0/2)
{ kind: 'key',    key, ctrl, ... }                           from DOM keydown (BT keyboard, uinput-replayed Enter/M)
{ kind: 'touch',  target }                                   row taps, picker cells
{ kind: 'tick',   name }                                     timer fired (esc-hold, toast-expire, enter-swallow-expire)
```

The knob-push double delivery (bridge `button` code 28 plus replayed DOM Enter) is documented in one place (the input layer) and the guard is a state, not a timestamp.

Screen/route model. A screen is data: `{ id, title, rows(state) -> Row[], hint(state), onEnter, onLeave, handlers }` where a row is `{ id, kind: 'label'|'nav'|'toggle'|'range'|'action'|'field', label, value?, target?, activate? }` and `activate` is a description of an effect (`{ send: {type:'wifi_scan'} }`, `{ push: 'wifi-list' }`, `{ pop: true }`, `{ term: {fontSize: 14} }`), not a closure that performs it. Navigation is a stack of screen ids with a per-screen highlight map, same as today (`state.menu.stack`/`highlight`), which is already the right model; the change is that it becomes serializable and its transitions pure.

Transport abstraction. One `Channel` per socket:

```
createChannel({ url, WebSocketImpl, timers, backoff }) -> {
  send(msg) -> 'sent' | 'dropped',   // explicit result, optional bounded queue for idempotent getters
  onMessage(fn), onStatus(fn),       // 'connecting' | 'live' | 'disconnected'
  close()
}
```

with injected `WebSocketImpl` and timers so node tests drive it with a fake socket. On top of it, typed `bridgeClient` and `agentClient` map wire messages to app events and expose `send.wifiScan()`-style helpers, so no call site builds a JSON literal.

## 3. Options

Options 1, 2, 3 vary the logic architecture; option 4 varies the view layer; option 5 varies the delivery. They are not mutually exclusive: the recommendation is a composition (section 5). Where an option is orthogonal to the rest I say so.

### Option 1: feature modules + small event bus, classic scripts, no build

Idea: split `app.js` into feature folders, each a classic script using the same dual-load wrapper as `wifi-logic.js`, communicating through a 30-line pub/sub bus and constructor-injected dependencies. Same runtime model as today, just partitioned.

Target tree:

```
frontend/
  index.html                  script list in dependency order
  style.css                   unchanged (optionally split per feature later)
  vendor/                     unchanged
  src/
    boot.js                   composition root: create channels, bus, views; wire everything; the only file that touches window globals
    core/
      module.js               tiny helper: define(name, factory) for UMD dual-load (browser: TR[name]; node: module.exports)
      bus.js                  emit/on/off pub/sub, no DOM
      config.js               URLs, key codes, timing constants (ESC_HOLD_MS, TOAST_MS)
      clock-source.js         injectable now()/setTimeout/clearTimeout facade (tests)
    transport/
      channel.js              reconnecting JSON WebSocket with backoff, status events, injected WebSocket
      agent-client.js         terminal-agent messages <-> bus events, typed send helpers
      bridge-client.js        controller-agent messages <-> bus events, typed send helpers
    input/
      buttons.js              button msg -> semantic actions; ESC tap/hold detection (state machine inside)
      keyboard.js             pure keydown -> PTY byte sequence (KEY_SEQUENCES, ctrl mapping) + thin DOM binder
      focus.js                focusScreen, refocus policy (DOM)
    terminal/
      sessions.js             session list, active index, dead map, byte-capped buffers (pure)
      terminal-view.js        xterm instance, fit, replay, scrollLines (DOM + xterm)
      tabs-view.js            tab bar render (DOM)
    menu/
      navigator.js            stack, highlight, adjusting, push/pop/popTo with onEnter/onLeave hooks (pure)
      menu-controller.js      knob/select/back semantics over navigator and active screen (pure; effects injected)
      menu-view.js            list/title/toast/hint rendering (DOM)
      screens/index.js        screen registry
      screens/root.js         root rows, brightness range
      screens/bluetooth.js    bluetooth, paired list, actions, pair list, confirm, passkey screens
      screens/terminal.js     font size, font family
      screens/timezone.js     groups, zones, grouping helpers
      screens/wifi.js         wifi screens + selectNetwork/submitPassword/startPasswordEdit
    wifi/wifi-logic.js        moved unchanged (pure)
    clock/clock.js            offset math (pure) + status-bar render
    status/status-view.js     agent/bridge dots
  tests/
    channel.test.js navigator.test.js menu-controller.test.js buttons.test.js sessions.test.js keyboard.test.js screens.test.js wifi-logic.test.js
```

Mapping from current code: `app.js:17-60` to `core/config.js`; `62-109` split into per-module state (sessions state to `terminal/sessions.js`, menu state to `menu/*`, wifi to `screens/wifi.js`); `111-152` to `terminal-view.js`; `154-220` to `tabs-view.js`/`sessions.js`/channels; `222-316` to `transport/*`; `318-384` to `input/buttons.js`; `386-526` split by prefix into `bridge-client.js` (decode) plus per-screen `onMessage` handlers; `528-558` to `terminal-view.js`/`sessions.js`; `560-574` to `clock/`; `623-997` to `menu/screens/*`; `1027-1076` to `navigator.js`; `1222-1318` to `menu-controller.js`; `1330-1456` to `menu-view.js` + `hints` per screen; `1458-1523` to `input/keyboard.js` + `focus.js`; `1525-1573` to `boot.js`.

Key abstractions: bus topics (`bridge:message`, `input:knob`, `input:button`, `menu:open`, `agent:output`), screen registry, `Channel`.

Testing: everything in `core`, `transport` (fake WebSocket), `input/buttons` (fake timers), `terminal/sessions`, `menu/navigator`, `menu/menu-controller`, `screens/*` row builders (if rows are data) and `keyboard` pure mapping is node-testable with no DOM. DOM stays in `*-view.js`, `focus.js`, `terminal-view.js`, `boot.js` (about 350 lines, 20%).

Footprint and build: no build, no new runtime deps; about 22 to 26 small files, about the same JS size (+2 to 4 KB for wrappers and bus). Needs `index.html` ordered `<script>` list and the recipe changed to install a directory (`SRC_URI = "file://src"` plus `cp -r`), otherwise every new file is a two-place edit. Load order is hand-maintained and enforced only at runtime, which is the main weakness (missing dependency shows as `undefined` at boot).

Pros: zero toolchain change, proven pattern in this repo, smallest conceptual jump, immediate testability of most logic. Cons: a bus makes control flow implicit (who reacts to `input:button`?), which is the exact problem now in a different costume; UMD wrapper boilerplate per file; global namespace and manual script ordering; does not by itself fix the flag-and-timer state encoding or the screen-closure side effects.

Effort: M. Risk: low to medium (mechanical moves, but no behavior net exists today, so each move needs the mock-WebSocket headless run recommended in section 4).

Migration, each step keeps the kiosk working:
1. Add `protocol/` fixtures and a mock-bridge replay test harness first (section 6), so every later step has a regression check. Nothing in `frontend/` changes.
2. Change recipe/`build-image.sh` to install `frontend/` minus `tests/` as a directory (one recipe change, then future file additions are free).
3. Extract `core/config.js`, `core/bus.js`; no behavior change.
4. Extract `transport/channel.js` and the two clients; replace `connectAgent/connectBridge`. Add backoff and explicit send result here.
5. Extract `terminal/` (sessions + view), cap buffers by bytes.
6. Extract `menu/navigator.js`, `menu-controller.js`, `menu-view.js`, then move screens one file at a time (timezone, terminal, bluetooth, wifi).
7. Extract `input/` last (ESC hold and Enter-swallow are the riskiest; move them with tests first).
8. Delete `app.js`, `boot.js` remains.

### Option 2: statechart-driven interaction layer (pure transitions, effects as data)

Idea: model the knob/button/menu interaction as an explicit hierarchical state machine with pure `transition(state, event) -> { state, effects }`. Timers and sockets are effects emitted as data and executed by a small interpreter. The screen tree (already declarative) stays, but rows return effect descriptions. This option addresses the highest-risk logic directly; it changes control flow, not file layout, so it composes with options 1, 3 and 5.

What the states are, concretely (derived from `app.js:318-384`, `1222-1318`, `1259-1266`):

```
terminal                                  (menu closed)
  ESC down -> open.list ; button 1..4 -> effect switchSession(n) ; knob -> effect scroll
open                                      (menu open; keydown to PTY suppressed here, replaces app.js:1488)
  list                                    knob moves highlight; ESC down -> escArmed
  escArmed                                effect startTimer('esc-hold', 450); ESC up -> select (tap); timer fired -> back (hold)
  adjusting                               knob changes value; select/back leaves
  editingText                             knob ignored; Enter/ESC/blur -> swallowEnter
  swallowEnter                            effect startTimer('enter-guard', 400); Enter ignored; timer fired -> list
  picker                                  knob moves ring; select applies
  pairing.confirm | pairing.passkey       entered from bridge events even when menu closed
```

Target tree (assumes classic-script delivery of option 1; shown relative to `src/`):

```
src/interaction/
  machine.js          createMachine({initial, states}) -> transition(); ~60 lines hand-rolled, no library
  menu-chart.js       the statechart above as a data table
  effects.js          interpreter: send(bridge msg), startTimer/cancelTimer, scroll, switchSession, focus, toast
  events.js           event constructors: knobEvent, buttonEvent, keyEvent, timerFired, bridgeMessage
src/menu/screens/*.js rows(state) -> Row[] with `activate` as effect descriptors
src/menu/menu-view.js render(menuState) (pure data in, DOM out)
tests/menu-chart.test.js   table-driven: [state, event] -> [state, effects]
```

Mapping: `handleButton` + `menuKnob/menuSelect/menuBack` + the `editFinishedAt` guard + the `adjusting`/`editing` booleans collapse into `menu-chart.js`; `handleSettingsEvent`'s navigation side effects (pairing pops/pushes) become transitions on bridge events; `menuHintText` becomes a function of the current state path plus screen (hint per state, not an if-chain on ids).

Abstractions: state path (`open.escArmed`), event, effect descriptor, `timerFired` as an ordinary event (so tests need no real timers).

Testing: the entire interaction layer becomes node-testable as a table of `(state, event) -> (state, effects)`, including the ESC tap vs hold race (`ESC down, ESC up before timer`, `ESC down, timer, ESC up`, `ESC down, menu closed externally`) and the Enter double-delivery case, which today can only be checked on hardware. Effects interpreter tested with fakes.

Footprint: +about 150 lines of machinery, no dependency. Do not vendor XState: v5 is ESM (blocked on `file://` without a bundler) and 40 KB+ for a menu of this size is overkill; a table plus a 60-line reducer is enough.

Pros: removes the most fragile logic (timers and flags), makes input remapping (a recurring change) a table edit, naturally documents the knob/button contract the docs currently describe in prose (`settings-menu-feasibility.md` sections 2.3, 12.1). Cons: statechart discipline can sprawl (hierarchical states in a 4-level menu); developers must learn the table; if applied alone it leaves the wire-message-to-state mutation and DOM coupling as is; it needs a state for "wire arrival during X" that is easy to get wrong (pairing request while editing text).

Effort: M (chart) to L (if also converting all rows to effect descriptors in one go). Risk: medium, concentrated in input semantics, mitigated by the table tests being written before the rewrite from today's observed behavior.

Migration: 1) write table tests encoding current behavior of ESC tap/hold/idle and knob push from reading `app.js`; 2) implement `machine.js` + chart behind the existing handlers, running both and logging disagreement in a dev build (shadow mode); 3) switch `handleButton`/`menuKnob`/`menuSelect` to call the chart and interpret effects; 4) convert rows to effect descriptors one screen at a time; 5) remove old flags (`escHoldTimer`, `editFinishedAt`, `adjusting`, `wifi.editing`).

### Option 3: unidirectional data flow (single store, reducers, effects, render(state))

Idea: Elm/Redux shape for the whole app. Every wire message and input event becomes an action; reducers produce the next state; an effects layer performs sends and timers; views are pure functions `render(state)` that update the DOM idempotently. Option 2's chart lives inside the menu reducer if combined.

Target tree:

```
frontend/src/
  main.js                   composition: store + effects + views + channels
  store/
    store.js                createStore(reducer, initial, runEffects); dispatch; subscribe; rAF-batched notify (~40 lines)
    actions.js              action creators / type constants, 1:1 with wire message types plus input actions
    reducer.js              combineReducers
    reducers/
      connection.js         agent/bridge status
      sessions.js           sessions, active, dead, clock offset/status
      terminal-output.js    per-session buffers (bytes-capped); xterm writes are an effect, not state
      menu.js               stack, highlight, adjusting, toast, escape/timer states
      bluetooth.js wifi.js brightness.js timezone.js terminal-settings.js
    selectors.js            currentScreen, currentRows, hint, status text
  effects/
    runner.js               executes effect descriptors returned by reducers/thunks
    send.js                 bridge/agent sends via channels
    timers.js               start/cancel by name, re-dispatches tick actions
    xterm.js                write, reset+replay, scroll, fit, option changes
  screens/                  rows(state) -> Row[]; title/hint/onEnter/onLeave as pure data (same registry as option 1)
  transport/ channel.js agent-client.js bridge-client.js     (as in option 1)
  views/
    menu-view.js tabs-view.js status-view.js clock-view.js terminal-view.js    render(state) only; no state of their own
  input/ buttons.js keyboard.js focus.js                      normalize to actions
frontend/tests/ reducers/*.test.js selectors.test.js store.test.js effects.test.js replay.test.js
```

Mapping: `state` (62-109) becomes the store's initial state sliced per reducer; `handleSettingsEvent` becomes the per-domain reducers (each `case` is already nearly a reducer branch, `app.js:394-524`); `menuSelect/menuKnob/menuBack/push/pop` become `menu` reducer cases; `renderMenu/renderTabs/renderClock/setStatus` become `views/*`; `sendBridge` calls become effects returned alongside state.

Key abstraction gain: replay. Record a bridge WebSocket trace on the device (all JSON messages with timestamps), feed it to the reducer in node, assert the resulting state sequence. That gives the regression net `docs/wifi.md:145` says is missing, and lets protocol fixtures from `protocol/` (section 6) double as reducer inputs.

Testing: all reducers, selectors and effect descriptors are pure and node-testable; only `views/*` and `effects/xterm.js` need a DOM or browser. Roughly 85% of lines testable without DOM.

Footprint: +about 250 lines of store plumbing, no dependency; views re-render on each state change, so use rAF batching and per-view `shouldUpdate` checks to avoid rebuilding the menu list at knob-spin rates (a fast knob spin sends many `knob` messages). Higher peak allocation than today only if views rebuild naively.

Pros: one place for state, the most testable architecture, time-travel/replay, reducers map 1:1 to wire messages, easiest to extend for per-tab activity, idle overlay (idle is just state plus a tick) and dashboards. Cons: most boilerplate; for 1500 lines it is the heaviest ceremony; effect modeling is the usual footgun (async arrival while a screen has been popped); everyone must internalize the discipline; the re-render design for xterm (imperative sink, not state) needs care so output chunks are not stored twice (state plus xterm) on a 512 MB device.

Effort: L. Risk: medium to high for a big-bang, low if migrated slice by slice behind the old code.

Migration: 1) store + `connection` and `sessions` reducers, views read from store, old code still owns menu; 2) move wire-to-state (`handleSettingsEvent`) into domain reducers, still calling old `renderMenu`; 3) menu reducer for navigation (with option 2's chart inside); 4) screens to row data; 5) delete globals.

### Option 4: component/view layer: Web Components or a vendored micro-library

Idea: replace imperative `innerHTML` rebuilds with declarative components. Two variants, both avoiding a bundler:

4a. Native Web Components (`<tr-menu>`, `<tr-tab-bar>`, `<tr-status-bar>`, `<tr-clock>`, `<tr-wifi-picker>`), vanilla custom elements, no library. Classic scripts can `customElements.define`. Zero footprint, works on `file://`. Each element gets `set state(v)`/`render()`; shadow DOM would break xterm's CSS and the global `style.css`, so use light DOM only (no encapsulation, which removes half the benefit).

4b. Preact + htm vendored as UMD (`preact.umd.js` about 4 KB gzipped, `htm.umd.js` under 1 KB; confirm the UMD artifacts exist for the pinned versions at vendoring time, I did not download them here). Gives real diffing, keyed lists and controlled inputs, which would fix the focus-stealing rebuild workaround for the Wi-Fi `<input>`. lit-html is ESM-only so it would force a bundler.

Target tree (4b, with option 1's logic split underneath):

```
frontend/src/ ... (logic modules as option 1 or 3)
  ui/
    app.js                  root component, mounts at #app
    MenuOverlay.js          list, row, toggle, range, signal bars, dot (htm templates)
    WifiPicker.js           ring strip, taps
    TextField.js            controlled password input (focus-safe)
    TabBar.js StatusBar.js Clock.js
    TerminalHost.js         ref wrapper around the imperative xterm instance (xterm stays imperative)
  vendor/ preact.umd.js hooks.umd.js htm.umd.js xterm*.js
```

Mapping: `renderMenu` (88 lines), `renderPicker` (30), `renderTabs` (10), status/clock setters (about 30) become components; roughly 160 lines of imperative DOM code replaced by 200 to 300 lines of templates.

Testing: components still need a DOM (jsdom is a dev dependency and adds a toolchain). Logic is not made more testable by this option; it only affects views. Honest take: the view layer is about 160 lines out of 1573 and is not where the bugs or the growth are. The Wi-Fi input focus workaround is the single real view-layer wart.

Footprint: 4a none; 4b +about 10 KB gzipped parse cost, a second vendored dependency with its own upgrade tracking, no JSX so htm tagged templates with no editor support.

Pros: declarative views; tidier growth if dashboards arrive (widgets, lists, charts); fixes the input-focus wart. Cons: solves the smaller problem; xterm still imperative; extra vendored dependency to keep pinned and re-verify against the device Chromium; light-DOM Web Components give little over functions; no logic testability gain.

Effort: M (4a) to M/L (4b). Risk: low to medium. Verdict: overkill for the current feature set. Revisit only if a dashboard-style multi-widget view is committed.

### Option 5: TypeScript (or JSDoc-typed JS) with an esbuild bundle producing one classic file

Idea: write real ES modules (`import/export`), bundle with esbuild to a single IIFE `app.js` (the form verified to work on `file://`), keep xterm as a vendored UMD classic script. TypeScript proper, or `// @ts-check` + JSDoc typedefs run through `tsc --noEmit` and no emit step at all.

Target tree:

```
frontend/
  package.json  package-lock.json       devDependencies: esbuild (pinned), typescript (optional)
  tools/build.mjs                       esbuild API call: bundle src/main.js -> dist/app.js, --format=iife --target=chrome68 (match device), sourcemap; copies index.html, style.css, vendor/
  src/                                  same module layout as option 1 or 3, but ESM (`import {createChannel} from '../transport/channel.js'`), .ts or .js+JSDoc
    protocol/types.ts (or .d.ts)        message types (generated or handwritten, see section 6)
  public/index.html style.css vendor/
  dist/                                 generated, gitignored; this is what build-image.sh rsyncs
  tests/                                node --test on the ESM sources directly (no UMD wrapper needed)
```

Build impact (candid):

- `build-image.sh` step 4 changes from `rsync frontend/` to `npm ci && node tools/build.mjs && rsync frontend/dist/`. The build host (`devel`) has node, and the step runs before kas/bitbake, so network for `npm ci` is available there (the "no network at build time" constraint is about the device and Yocto's do_compile, not about this pre-step). Offline reproducibility needs a lockfile and either a pinned `esbuild` tarball cached in `~/.npm` or a vendored binary. The Yocto recipe itself stays dumb (installs a fixed `dist/` file list), which is a plus.
- CI gains `npm ci`, typecheck, `node --test`, build, and a size check; Node 20 in CI should be bumped to 22+ if tests import `.ts` natively.
- Developer loop: edit, `npm run build` (13 ms measured), scp `dist/`. The documented "hand-edited, scp-deployed, no build by design" workflow (`ci.yml:63-65`) is replaced. The deployed `app.js` is no longer the file you edited; use sourcemaps (remote-debugging port works for that) or accept minified stack traces on the device.
- Supply chain and drift: one devDependency tree of about 12 MB on disk (esbuild plus typescript more), 2 packages to keep patched; generated artifacts must never be committed or they drift.
- Syntax target: with the device on Chromium 68, a bundler step (esbuild with `--target=chrome68`) is the only way to write modern syntax such as `?.` and `??` and have it downleveled. Classic scripts do not get that safety net and must stay inside the Chromium 68 syntax ceiling (section 1.6). esbuild downlevels syntax only; it does not polyfill runtime APIs such as `Array.prototype.flat` or `Object.fromEntries`.

Mapping: same logical split as option 1 or 3; the difference is `import` replaces UMD wrappers and global namespace, and load order is solved by the module graph.

Testing: node imports the ESM sources directly; types catch wire-message shape drift if generated from the protocol spec.

Footprint at runtime: smallest (tree-shaken, minified 25 KB for today's code vs 62 KB), but the saving is irrelevant next to xterm.js 283 KB and Chromium's baseline.

Pros: real modules, import graph, types, no global namespace, no manual script order or recipe edits, best long-term fit if the codebase triples or a UI library enters. Cons: introduces the only build step and toolchain in the frontend, changes the deploy and debug workflow, adds a lockfile and Node bump to CI, risk of two sources of truth if `dist/` is committed, and buys mostly ergonomics at 1.5K lines. TypeScript specifically adds a type layer for a team of one maintaining about 1500 lines; JSDoc plus `tsc --noEmit` gets about 80% of the safety with no emit.

Effort: M for the build plumbing, plus whatever logic restructure is done on top (so M to L total). Risk: medium (workflow change), but the code risk is low since the bundle is semantically the same as today's classic script.

Alternative delivery variants considered and rejected for this option:

- Add `--allow-file-access-from-files` and use native ESM with no build: lowest effort, but the flag lives in upstream layer territory, changes security posture, and a failure mode is a blank screen (verified: with default flags every module import is blocked, section 1.4). Acceptable only if the project already owns the launcher unit. Not verified here.
- Serve over `http://localhost` from `controller-agent`/`terminal-agent`: adds an HTTP server and a new origin to two Rust daemons for a frontend ergonomics gain. No.
- Hand-rolled concatenation script (cat files in manifest order into one IIFE): a zero-dependency build step that reimplements a bad esbuild. If a build step is going to exist, use esbuild.

## 4. Comparison matrix

Scores 1 (worst) to 5 (best) against this project's constraints. "Testable" = share of logic reachable in node without a DOM.

| Criterion | 1 Modules + bus (classic) | 2 Statechart (interaction only) | 3 Store/reducers/effects | 4 Components / micro-lib | 5 ESM + esbuild (TS/JSDoc) |
|---|---|---|---|---|---|
| Fixes the real pain (flags/timers, screen closures, wire handling) | 2 | 5 | 5 | 1 | 2 (packaging only) |
| Node-testable share after migration | about 60% | interaction about 100%, rest unchanged | about 85% | unchanged | depends on logic option underneath |
| Works on file:// with current launch flags | yes | yes | yes | yes (4a, 4b via UMD) | yes (single IIFE) |
| New build/toolchain | none | none | none | none (vendored libs for 4b) | esbuild + lockfile + CI changes |
| Runtime footprint delta | +2-4 KB | +5 KB | +8-10 KB | 0 / +10 KB gz | -37 KB (minified) |
| Boilerplate | medium (UMD per file) | low-medium | high | medium | low (once set up) |
| Fit for backlog features (more screens, idle overlay, tab activity, keyboard fallback) | 3 | 4 | 5 | 3 | 4 |
| Fit for a possible dashboard future | 2 | 2 | 4 | 5 | 5 |
| Migration risk (kiosk keeps working per step) | low | medium | medium | low | medium (workflow) |
| Effort | M | M-L | L | M | M-L |
| Overkill for ~1.5K lines? | no | no | borderline | yes | borderline |

Weighted view (weights chosen from the constraints): pain fixed 30%, no new toolchain 20%, testability 20%, effort/risk 15%, future fit 15%. Result: option 2 plus option 3's selectors/reducer style for the pure core scores highest; option 1's file split is the necessary vehicle; options 4 and 5 score below because they do not address pain and add dependencies respectively.

## 5. Recommendation

Verdict: build a pure interaction core in the style of option 2 (explicit state machine, effects as data) inside option 3's minimal store shape (one reducer for domain state, selectors for views), split into option 1's feature folders, delivered as classic scripts with the existing UMD dual-load pattern. No bundler yet. Add a `protocol/` contract with fixtures (section 6) before moving any code. In concrete terms:

```
frontend/
  index.html style.css vendor/                (unchanged)
  src/
    boot.js                                   composition root
    core/        module.js bus.js config.js timers.js
    transport/   channel.js agent-client.js bridge-client.js
    state/       store.js reducers.js selectors.js         (small: domain data only; menu navigation via interaction/)
    interaction/ machine.js menu-chart.js effects.js events.js
    input/       buttons.js keyboard.js focus.js
    terminal/    sessions.js terminal-view.js tabs-view.js
    menu/        screens/*.js (rows as data) menu-view.js
    wifi/        wifi-logic.js
    clock/       clock.js
  tests/         (node --test, wired into CI)
protocol/        schema + fixtures (shared with controller/ and daemon/)
```

Reasoning, with the alternatives explicitly weighed:

- Why not just option 1: a bus plus file split moves the same flag-and-timer logic into smaller files. The ESC hold state and Enter swallow stay untestable in practice. Option 1 is the right container, not the fix.
- Why not full option 3: the terminal half (`output` chunks, scroll, fit) is an imperative sink that should not be in the store (it would double memory for terminal bytes on a 512 MB device), and most menu "state" is only meaningful as a navigation machine. Taking the store only for domain data (bt/wifi/brightness/timezone/connection/sessions) plus the chart for interaction gets the testability without the ceremony. If the team prefers one mechanism, a full store is acceptable, just slower to land.
- Why not option 4: the view layer is 160 lines and is not the problem. The one real view wart (Wi-Fi input focus) is fixable by keeping the existing out-of-list input. Revisit if dashboards become a committed feature.
- Why not option 5 now: the cost is a permanent toolchain and workflow change for a gain that is mostly ergonomic at this size. Verified facts: ESM needs a bundler or a launcher flag under `file://`, esbuild would work cheaply, but classic scripts with a dual-load wrapper (already proven by `wifi-logic.js` and its tests) cover the actual need. The honest cost of staying classic is about 25 files with hand-ordered `<script>` tags and a 5-line wrapper each; that is annoying but bounded.

Triggers to adopt esbuild (option 5 delivery) later, so the decision is not left as vague "maybe":

1. A UI library or any ESM-only dependency enters (dashboards, lit, charts).
2. Module count passes about 30 or load-order bugs occur twice.
3. TypeScript is wanted for generated protocol types (section 6) and JSDoc becomes painful.
4. The Chromium target is pinned below what the code uses and downleveling is needed.

Because modules are written with the dual-load wrapper and dependency injection, moving to ESM later is a mechanical `sed` (wrapper to `export`), not a rewrite. That is the main reason this recommendation is cheap to reverse.

What NOT to do:

- Do not add a bundler, npm tree or TypeScript as step one just because it is fashionable; the first benefit comes from restructuring logic, not tooling.
- Do not vendor XState or Redux (both ESM-first in current versions, size and a vendored upgrade surface for a 50-line need).
- Do not store terminal output in a reactive store; keep xterm as the single owner of terminal bytes (plus one byte-capped replay buffer per session).
- Do not use shadow DOM Web Components; they break xterm styling and the global stylesheet.
- Do not rely on `--allow-file-access-from-files` without owning and testing the launcher unit.
- Do not commit generated `dist/` or generated types without a CI check that regeneration is clean.
- Do not do a big-bang rewrite; the kiosk has no automated end-to-end net today, so build the net first.

Recommended migration order (kiosk working after each step, all steps independently shippable):

1. Contract and net first (S): `protocol/` schema and fixtures, a node test that replays them through the existing `handleSettingsEvent`-equivalent is not possible yet, so write a committed headless-Chromium mock-WebSocket script (the one `docs/wifi.md:145` says is not committed) driving the current app: open menu, BT list, brightness adjust, Wi-Fi connect flow, ESC tap/hold, tab switch. Add `node --test frontend/tests/*.test.js` to CI (fix the directory invocation).
2. Recipe installs `src/` as a directory; `build-image.sh` skips `tests/` (S).
3. Extract `core/config.js`, `transport/channel.js` + clients with backoff and explicit send results (S/M). Keep the syntax ban in `app.js:1-8`: the device runs Chromium 68 (owner-confirmed 2026-11-22), which predates `?.` and `??`. It can only be lifted behind an esbuild step with `--target=chrome68`.
4. Extract terminal modules and cap replay buffers by bytes (S).
5. Write `interaction/` machine and table tests for ESC/knob/Enter semantics first, in shadow mode against the live handlers, then switch over (M, the high-value step).
6. Convert screens to row-data one file at a time, hint text per screen/state; remove wifi special cases from generic menu code (M).
7. Introduce the small domain store if the per-module state starts to need cross-feature reads (idle overlay, tab activity). Skip if not needed (optional).
8. Delete `app.js`.

Confidence: high on diagnosis and on "no bundler yet", medium on the exact split (file granularity is adjustable), medium-low on future-fit claims for dashboards because they are not actually in the backlog. What would change my mind: a committed dashboard feature, an ESM-only dependency, or the kiosk launcher already being under this repo's control (then native ESM with the flag becomes the cheapest module story, though I would still prefer bundling for the safety net).

Steelman of the opposite view (go to option 5 now): file count and manual load order are exactly what bundlers solve; the repo's owner values clean separation and spec-first types, which are far more pleasant with TS and generated types; the build host already has node. This is a reasonable position. It loses on cost-vs-benefit today and on the deliberate "no build step" policy stated in `README.md`, `ci.yml:63` and `docs/terminal.md`, and it is the easier of the two to adopt later, so postponing is cheaper than reversing.

## 6. Shared concerns with the controller/ and daemon/ analyses

### 6.1 What the wire protocol looks like today

Two JSON-over-WebSocket protocols, tagged by `type`.

- Controller bridge, port 8765. Server to client (Rust `enum BridgeEvent`, `#[serde(tag = "type")]`, `controller/src/main.rs:79-84`): `button {code, value}`, `knob {delta}`, `bt_power`, `bt_error`, `bt_scan_started`, `bt_scan_stopped`, `bt_device_found`, `bt_paired_list`, `bt_pairing_confirm_request`, `bt_pairing_passkey_display`, `bt_pair_result`, `brightness {value, max}`, `timezone`, `timezone_list`, `settings_error`, `wifi_available`, `wifi_status`, `wifi_scan_started`, `wifi_scan_results`, `wifi_connecting`, `wifi_connect_result`, `wifi_saved_list`, `wifi_error`. Client to server (`enum ClientCommand`, `main.rs:283-285`): the 22 commands listed in 1.2. The Rust doc comment (`main.rs:76-78`) says "field shapes here are the source of truth", and `docs/wifi.md` also holds a hand-written table of the same shapes. Unknown input is ignored by design (`main.rs:280-282`).
- Terminal agent, port 8766 (`docs/terminal.md` "Wire protocol"): `session_list`, `output`, `exited`, `clock_sync` out; `input`, `resize`, `control` in.

The frontend currently re-derives all of this by hand in `app.js` (field names appear as string literals at about 60 sites) and Rust has per-enum unit tests that parse JSON literals (`controller/src/main.rs:941-980`). There are therefore at least three descriptions (Rust types, markdown tables, JS literals) with nothing checking they agree.

### 6.2 Is spec-first generation worth it at this size?

Honest assessment: about 45 message types across two protocols, one maintainer, three consumers (JS, two Rust crates). Full TypeSpec-driven generation of JS types and Rust types would work in principle, but:

- TypeSpec's strengths are HTTP/REST and its emitters (OpenAPI, JSON Schema); first-class WebSocket/event-stream message modelling is thin, so you would use TypeSpec models only as a JSON Schema generator and hand-assemble the `oneOf` envelope by `type` discriminator. I have not verified the current state of any AsyncAPI emitter; treat that as unknown and check before committing.
- Rust-side: generating `serde` enums from JSON Schema (e.g. typify) tends to produce verbose types that do not match the existing hand-tuned ones (`Secret` wrapper that redacts passwords in `Debug`, `#[serde(default)]`, forward-compatible ignoring). Going the other way (schemars from the Rust enums to JSON Schema) keeps the Rust code idiomatic and makes Rust the source of truth, which is the opposite of spec-first.
- JS-side: generated `.d.ts` is only valuable if the frontend uses TS or `@ts-check`; with plain classic scripts the benefit is IDE hints plus a test.
- Adds a generator toolchain (node TypeSpec compiler, cargo build.rs or codegen step) to three build paths for 45 messages. It does not pay for itself at this size.

So full codegen: not worth it now. The spec-first intent is worth keeping through a lighter mechanism that still makes the spec the thing both sides are tested against.

### 6.3 Minimal useful version (recommended): schema plus fixtures, checked on both sides

Create a repo-root `protocol/` owned jointly:

```
protocol/
  README.md                    conventions: `type` discriminator, naming, nullable fields, forward-compat rule (ignore unknown), secrets rule (password never echoed)
  bridge.schema.json           JSON Schema 2020-12: $defs per message, `oneOf` for client->server and server->client (discriminator: `type`)
  terminal.schema.json         same for port 8766
  fixtures/
    bridge/client/*.json       one valid example per command, plus a few invalid ones in fixtures/bridge/invalid/
    bridge/server/*.json       one valid example per event
    terminal/{client,server}/*.json
  VERSION                      integer; bump on breaking change (optional `hello {protocol: n}` message later, to be decided with siblings)
```

Checks (all cheap, all in existing CI jobs):

- Rust (controller): a test that iterates `fixtures/bridge/client/*.json` and asserts `serde_json::from_str::<ClientCommand>` succeeds, `fixtures/bridge/invalid/*.json` fails, and that serializing a constructed `BridgeEvent` for each server fixture equals the fixture JSON. Rust already tests this shape for Wi-Fi (`main.rs:941-1004`); this generalizes it. Same for daemon.
- Node (frontend): a test that validates every fixture against the schema with a tiny hand validator (about 60 lines: required keys, primitive types, enum values, nullable) or ajv as a dev-only dependency (acceptable only in a future build option); and a frontend contract test that feeds every server fixture to `bridge-client.js`/`agent-client.js` and asserts a decoded app event (nothing unknown/dropped), and that every `send.*` helper emits a message that matches a client fixture.
- Docs: `docs/wifi.md` and `docs/terminal.md` protocol tables replaced by a link to `protocol/` plus a generated or checked table (a script that fails CI if the markdown table and schema disagree is optional).

JSDoc/TS types from the schema are then a later add-on: when the frontend adopts TS or `@ts-check`, generate `protocol/types.d.ts` with `json-schema-to-typescript` as a one-shot checked-in file plus a CI "regeneration is clean" check. Rust types stay hand-written and fixture-verified. This delivers the main value of spec-first (one reviewable contract, drift caught by tests) without a generator in the Rust build.

If the owner wants real TypeSpec: the cheapest honest path is TypeSpec models, JSON Schema emitter output committed under `protocol/`, same fixtures and tests. The fixtures-and-tests layer is identical, so the choice between hand-written JSON Schema and TypeSpec-emitted JSON Schema can be deferred without rework.

### 6.4 Items to coordinate with the controller/ and daemon/ proposals

1. Directory and ownership of `protocol/`, naming convention (snake_case `type` values, `*_get`/`*_set` verbs, `*_result` for async outcomes), and whether the two protocols stay separate schemas (recommended) or merge.
2. Versioning: add a `hello`/`protocol_version` message or accept versionless. Frontend and daemons ship in one image so mismatch is a dev-time problem; a version field still makes fixture drift checkable.
3. Error envelope: today there are `bt_error`, `settings_error`, `wifi_error` with `{context, message}`. Unify under one `error {domain, context, message}` or keep and document; frontend wants one handler.
4. Send acknowledgement: commands are fire and forget and the frontend silently drops sends while disconnected (`app.js:210-220`). Decide whether a `get` on connect is replayed by the server (server-pushed initial state on connect would remove the four `*_get` sends at `app.js:284-287`).
5. Knob/button delivery contract: the bridge `button` plus uinput-replayed Enter double delivery is the reason for the 400 ms guard. Controller should document it in the schema README; if it ever changes (e.g. no replay for Enter when menu is open), the frontend state machine changes with it.
6. Secrets: `wifi_connect.password` is the only secret; the schema should mark it, and a fixture test should assert it never appears in any server event (the Rust side already asserts this for connect results, `docs/wifi.md:86`).
7. Binding and trust: both daemons bind `0.0.0.0` (`controller/src/main.rs:921`, `daemon/src/main.rs:839`) while `docs/terminal.md` describes a loopback trust model. With a Wi-Fi adapter now supported, the terminal agent (a shell over unauthenticated WebSocket) and the controller (Wi-Fi/BT commands) are reachable from the LAN. The frontend only needs `localhost`, so binding to `127.0.0.1` costs it nothing. This is a siblings concern but the frontend can confirm it is safe on its side. Flagging because it changes the threat model of the whole protocol design.
8. Timing constants shared across sides: ESC hold 450 ms (frontend) vs KEY_M 1500 ms (controller), knob delta semantics (frontend ignores magnitude for lists), `clock_sync` cadence and `age_s`. Put in `protocol/README.md` as the cross-component timing table.

## 7. Appendix: experiment log (all in `~/.hermes/cache/scratch/esm`, repo untouched)

- ES module test page: static import, dynamic import and inline module under `file://`; run with default flags, `--in-process-gpu`, and `--allow-file-access-from-files`, Chromium 156.0.8078.4 via playwright python. Results table in 1.4.
- esbuild 1 package: `npm i esbuild` (12 MB `node_modules`), `esbuild app.js --bundle --minify --format=iife --target=chrome120` (run before the Chromium 68 correction; use `--target=chrome68` for the device) on a copy of `frontend/`: 25.6 KB output in 13 ms (note: this bundles `app.js` alone, which does not import `wifi-logic.js`; with it loaded the combined size is within about 5 KB of this).
- `node --test frontend/tests/wifi-logic.test.js`: 10/10 pass on Node v24.22.3. `node --test frontend/tests/`: fails (module not found). `node a.ts` (type annotations): runs on Node 24.
- Not verified: the real kiosk launch flags on the device, the on-device Chromium version via `/json/version` (owner says 68; earlier text here assumed 147), availability and exact size of Preact/htm UMD builds, TypeSpec WebSocket/AsyncAPI emitter maturity.
