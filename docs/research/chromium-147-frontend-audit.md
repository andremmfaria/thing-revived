# Chromium 147 frontend audit

Date: 2026-11-22. Scope: `frontend/`, `config/frontend.toml`, `build/tools/toml_to_js.py`, the vendored xterm.js, and the syntax guard. Nothing in the code, tests or config was changed by this audit.

## Summary

The kiosk browser is Chromium 147, not 68 or 69. The frontend was written under a Chromium 68/69 ceiling, and that ceiling is now a self-imposed conservative baseline, not a browser limit. Almost every encoded constraint is a stale workaround: it costs nothing to keep, but nothing forces it. The one place where the old claim may still describe a real constraint is ES modules over `file://`, and that reason is stated wrongly in the repo (see section 4). The `syntax-guard` test enforces the wrong target and also rejects code that would work on the device.

Biggest finding for decisions: do not mass-rewrite. The code is plain `var` and `function` style (501 `var` occurrences under `frontend/src`, zero `const`, `let` or arrow functions in code), dual-loaded under node for tests. A modernisation sweep has real churn and no functional payoff. The recommended work is mostly doc and guard corrections plus a few optional, low-risk upgrades.

## 1. Verification status

### Verified on the device (read-only, 2026-11-22)

- `http://127.0.0.1:9223/json/version` reports `Chrome/147.0.0.0`, `V8-Version 14.7.173.20`, `WebKit 537.36 (@dbcf1b1bfb506cc580859bcb5ff9460a8443af90)`, user agent contains `CrKey/1.56.500000`.
- `/json/list` shows one page target, title `Terminal // Car Thing`, url `file:///usr/share/superbird-kiosk-default/index.html`.
- Process command line (from `/proc/*/cmdline`): `/usr/lib/chromium/chromium-bin` with `--no-sandbox --ozone-platform=wayland --use-angle=gles --cast-graphics-width=800 --cast-graphics-height=480 --enable-input --enable-wayland-ime --allow-insecure-localhost --single-process --num-raster-threads=4 --disable-low-end-device-mode --enable-zero-copy --js-flags=--max-old-space-size=192 --max-semi-space-size=16 --remote-debugging-port=9223 --remote-allow-origins=*` and the `file://` URL above. It runs as root. Notable absences: no `--allow-file-access-from-files`, no `--disable-web-security`.
- Because the page loads and renders today with Chromium 147 and the vendored xterm.js 5.3.0, that combination works at least for the current feature set.

### Not verified (needs a device test)

Every claim below about what a specific syntax, CSS feature or API does on this exact build is based on general knowledge of Chromium 147, not on a run on the device. A single build can lack features through flags (`--disable-features=...` lists many), so the table in section 3 should be proven with the script in section 8 before relying on it.

## 2. Where the old limit is encoded

Counts: the phrase `Plain ES2018 subset, classic script, dual-load` appears in a header comment of 32 of the 33 files under `frontend/src` (all except `wifi/wifi-logic.js`, which says `Plain ES6` at line 5).

| Location | What it says | Real constraint or stale | Notes |
|---|---|---|---|
| `frontend/index.html:66-67` | non-module classic script is required for a `file://` page load | Partly stale, partly unverified | See section 4. The browser version is not the reason; the file origin is. |
| `frontend/index.html:69-71` | no ES modules, no bundler, device runs Chromium 69 over `file://` | Stale version, constraint unverified | Wording should say Chromium 147. |
| `frontend/style.css:44-45` | `Chromium 69 has no flexbox gap`, spacing via `.tab-bar > * + * { margin-left: 4px; }` | Stale workaround | `gap` on flex is supported since Chromium 84. Equivalent rule is `.tab-bar { gap: 4px; }`. Only one such rule exists. |
| `frontend/src/utils/config.js:5` | kiosk cannot read TOML (no build step, `file://` page, Chromium 69) | Mixed | The real reason is that a `file://` page cannot `fetch()` a TOML file and no TOML parser ships; the version in the parenthesis is stale. `fetch` on `file://` is blocked in Chromium regardless of version (unverified on this build, see section 8). |
| `config/frontend.toml:2-3` | same sentence as `config.js:5` | Same | Comment only. |
| `build/tools/toml_to_js.py:4` | `Chromium 69, file://, no build step` | Same | Docstring only. The converter itself is a sound, deterministic generator and is not tied to a version. |
| `frontend/src/**` header comments (32 files, e.g. `boot.js:15`, `bus.js:6`, `channel.js:22`) | `Plain ES2018 subset` | Stale and inaccurate | The code is ES5 style plus a few ES2015 library calls (`Object.assign`, `Object.keys`, `filter`, `forEach`, `Array.isArray`; 62 lines use one of these). The label overstates the sophistication and understates what is allowed. |
| `frontend/src/wifi/wifi-logic.js:5` | `Plain ES6, same syntax limits ... (docs/research/architecture-proposals/frontend.md section 1.6)` | Stale | Points at the old cheat sheet that was written for Chromium 68. |
| `frontend/src/terminal/terminal-view.js:64` | `if (typeof ResizeObserver === 'function') new ResizeObserver(schedule).observe(el);` | Stale guard | `ResizeObserver` exists since Chromium 64, so the feature test was never needed on 68/69 either. Harmless. The guard also silently hides a regression if the API ever disappeared; the node test environment has no `ResizeObserver`, which is the real reason a guard may be useful (keeps the module loadable under node). Keep or drop is a style choice. |
| `frontend/src/terminal/sessions.js:7` | `sessions share 512 MB with Chromium` | Real constraint | Memory, not version. Keep. Also note the V8 flag `--max-old-space-size=192` on the live command line. |
| `frontend/src/utils/logging.js:2-3` | console output reachable through the remote debugging port | Real | Accurate. |
| `frontend/style.css:34` | `height: 100vh` | Fine | Works on 147. `100dvh` is not needed on a fixed 800x480 kiosk with no dynamic toolbars. |
| `frontend/tests/syntax-guard.test.js:2-4, 14, 24-25, 121-133, 146-149, 170, 177, 197-204, 220, 298, 314, 325` | Whole file is the 68/69 enforcement | Stale target | See section 6. |
| `frontend/tests/README.md` (Syntax guard section) | restates the 69 ceiling | Stale | Doc updated in this change. |
| `frontend/vendor/README.md` (Browser constraint) | restates the ceiling and the file:// reason | Stale and partly wrong | Doc updated in this change. |
| `docs/research/architecture-proposals/frontend.md:15, 118-122, 427` | the Chromium 68 cheat sheet and the esbuild `--target=chrome68` argument | Historical | Left unedited by instruction. The esbuild argument no longer applies (a bundler is not needed to reach modern syntax). |

### Things searched for and not found

No polyfills, no `Promise`, `fetch`, `async`, `await`, optional chaining, nullish coalescing, class fields, `structuredClone`, `Intl`, `inset`, `clamp()`, `aspect-ratio`, `min()`/`max()` in CSS, `:is()`, `:where()`, CSS nesting, container queries or `dvh` anywhere in `frontend/src`, `index.html` or `style.css` (the grep hits for `min(` and `max(` are `Math.min` and `Math.max`). The code avoids them because the guard forbids them, not because they were needed and worked around. The only genuine workaround for a missing feature is the flexbox `gap` margin at `style.css:44-45`. The WebSocket client (`transport/channel.js`) uses the native `WebSocket` constructor with a 16-message queue and a reconnect loop; WebSocket needs nothing newer than the current baseline.

Observed browser features the code relies on that are fine on 147: `WebSocket`, `ResizeObserver`, `requestAnimationFrame`, `addEventListener` in capture phase, `100vh` flex column layout, CSS custom properties (`var(--...)`), `text-shadow`, `radial-gradient`.

## 3. What could be modernised: risk and benefit

All entries are unverified on the device (section 1) unless stated. "Supported since" is the first Chromium release, from public compatibility data, and all are far below 147.

| Item | Supported since | Benefit | Risk | Verdict |
|---|---|---|---|---|
| Flexbox `gap` replacing `.tab-bar > * + * { margin-left }` (`style.css:44-45`) | 84 | One fewer rule, removes the misleading comment | Visual diff of 4px on the tab bar; verify with the e2e screenshot | Quick win, optional |
| `const` and `let` instead of `var` (501 sites) | far below 147 | Block scoping, readability | Large diff across 32 files, tests must keep loading under node (they do, node supports it); no runtime gain | Not worth a sweep. Allow in new code. |
| Arrow functions, template literals, destructuring, spread | far below 147 | Terser code | Same churn issue | Allow in new code only |
| `?.` and `??` | 80 | Less defensive code | None at runtime | Allow in new code once the guard is relaxed |
| `Promise`, `async`/`await`, `fetch` | far below 147 | Not needed today; code is event and timer driven | `fetch` of local files from `file://` is not usable (unverified, see section 8) | No use case. Do not add for its own sake. |
| `structuredClone` | 98 | Replaces hand-rolled deep copy if any | None found in use; reducers use `Object.assign` copies | No use case |
| ES modules (`<script type="module">`) | far below 147 | Real imports, removes `window.TR` namespace and manual script order | Likely blocked from `file://` (opaque origin, CORS); unverified; also breaks the dual-load (node CommonJS) trick the 33 files use; would need a build step or a served origin | Not worth it unless the page is served over `http://127.0.0.1` |
| CSS `inset`, `aspect-ratio`, `clamp()`, `min()`/`max()`, `:is()`, `:where()`, nesting | 87, 88, 79, 79, 88, 88, 120 | None needed for a fixed 800x480 screen | Churn with no visible effect | Not worth it |
| `dvh`, container queries | 108, 105 | Irrelevant on a fixed-size kiosk | None | Not worth it |
| Remove `typeof ResizeObserver` guard (`terminal-view.js:64`) | 64 | Cosmetic | The guard keeps the module loadable under node tests | Leave as is |
| Update header comments (32 files) | n/a | Truth in comments | Pure comment diff, easy to review | Quick win |

Memory note that does limit modernisation: the browser is `--single-process` with `--max-old-space-size=192` and a 512 MB device (`sessions.js:7`). Any addition of a large library should be tested for heap use.

## 4. ES modules over `file://`: what is known

- Claim in the repo (`frontend/vendor/README.md`): a `file://` page load has no Content-Type header, so `<script type="module">` fails to load. This is an imprecise explanation. Chromium treats `file://` as an opaque origin and applies CORS to module script fetches, so module scripts from `file://` are blocked with a CORS error (`origin 'null'`), unless `--allow-file-access-from-files` is passed. This is widely reported behaviour in Chromium, but it was not tested on this device.
- Verified on the device: the command line has no `--allow-file-access-from-files`. So the default behaviour is expected to apply.
- Not verified: that module scripts actually fail on this build. Relevant caveat: the kiosk passes `--allow-insecure-localhost`, which does not affect `file://`.
- Conclusion: the classic-script rule is probably still a real constraint for the current deployment (file origin), but the reason is the file origin, not the browser version, and the wording in the repo should be corrected. It stops being a constraint if the web app is served from `http://127.0.0.1` by an existing agent, which is a deployment change, not a frontend tweak.

Exact check (see section 8, script A): load a tiny module page from the same directory through a scratch target and read the console error.

## 5. Vendored xterm.js

Current: xterm.js 5.3.0 and xterm-addon-fit 0.8.0, UMD, 283371 bytes plus 1460 bytes. Contains no `?.` or `??` in the vendored 5.3.0 file (checked by grep). Both npm packages `xterm` and `xterm-addon-fit` are now deprecated ("Move to @xterm/xterm instead", checked with `npm view`).

Newer releases (npm, checked 2026-11-22): `@xterm/xterm` 6.0.0, `@xterm/addon-fit` 0.11.0, `@xterm/addon-webgl` 0.19.0, `@xterm/addon-canvas` 0.7.0 (last release 2024-07-14, for the 5.x line).

| Topic | Finding | Status |
|---|---|---|
| xterm 6.0.0 builds | The tarball still ships `lib/xterm.js` (UMD, 488.7 kB, `main`) and `lib/xterm.mjs` (ESM, 345.0 kB), so a classic-script vendor is still possible | Verified (tarball listing) |
| Syntax of 6.0.0 | The UMD build contains `?.` (211 hits) and `??` (51 hits). Fine on Chromium 147, would have failed on 69. It is outside the guard because `vendor/` is excluded | Verified by grep; runtime on device unverified |
| Canvas renderer | Removed in xterm 6.0.0 ("Remove the canvas renderer", PR 5105, breaking). `@xterm/addon-canvas` 0.7.0 targets 5.x only | Verified (release notes) |
| Viewport and scrollbar | 6.0.0 notes a potential breaking change: the scrollbar and viewport work very differently now, and `overviewRulerWidth` moved to `overviewRuler.width` | Verified (release notes). Impact on this app (knob scrolling uses `term.scrollLines`-style calls, `disableStdin: true`) is unverified |
| New in 6.0.0 | Synchronized output (DEC mode 2026), OSC 52, shadow DOM support in WebGL, ligature variants, `onWriteParsed`, textarea read-only when `disableStdin` | Verified (release notes). Benefit for this app is small |
| FitAddon 0.11.0 | Package is tiny (`lib/addon-fit.js` 1.5 kB). Drop-in for the same API | Verified (tarball); behaviour with 6.0.0 unverified |
| WebGL addon 0.19.0 | `lib/addon-webgl.js` 247.5 kB UMD. It requests a `webgl2` context (two `getContext("webgl2", ...)` calls found in the bundle). WebGL1 is not used | Verified by grep |
| WebGL on this device | Chromium runs with `--use-angle=gles` on a Mali-G31 (Panfrost) under Wayland with `--single-process`. Whether `WebGL2` context creation succeeds, is stable and is faster than the DOM renderer here is unknown. xterm falls back to the DOM renderer if the addon throws or the context is lost, if the app handles `onContextLoss` | Unverified. Needs the check in section 8 script B |
| Canvas renderer on this device | Only usable with xterm 5.x plus `@xterm/addon-canvas` 0.7.0. Canvas 2D is software or Skia-GPU depending on the build; no evidence either way | Unverified, and a dead end because the addon is gone in 6.x |
| Memory | WebGL texture atlas plus a 192 MB V8 heap cap and 512 MB total RAM shared with up to four PTY sessions | Unverified. Measure before adopting |
| Current renderer | The DOM renderer (default). With an 800x480 terminal and `disableStdin: true`, scroll and redraw cost is probably low | Unverified; no measurement exists in the repo |

Recommendation for xterm: staying on 5.3.0 is fine. If upgrading, go straight to `@xterm/xterm` 6.0.0 and `@xterm/addon-fit` 0.11.0 (the old package names are deprecated), keep the DOM renderer, hardware-test knob scrolling and resize, and only then try WebGL on the device behind a config flag with automatic fallback. Provenance steps in `frontend/vendor/README.md` would need new package names and URLs.

## 6. The syntax-guard test

What it does today (`frontend/tests/syntax-guard.test.js`, 329 lines): a custom lexer scans for syntax and APIs newer than Chromium 69, optionally parses with acorn at ES2019, runs `node --check`, and scans CSS for a ban list (lines 196-224). Its self-test (lines 261-282) pins the ban list.

Problems under Chromium 147:

- It bans things that work on 147: `?.`, `??`, logical assignment, class fields, private members, static blocks, `flat`, `flatMap`, `matchAll`, `fromEntries`, `allSettled`, `replaceAll`, `.at()`, `globalThis`, `structuredClone`, and the CSS list `inset`, `aspect-ratio`, `backdrop-filter`, `:is()`, `:where()`, `:focus-visible`, `prefers-color-scheme`, `clamp()`, `min()`, `max()`, flexbox gap.
- The acorn layer pins `ecmaVersion: 2019`.
- Its comments cite the wrong browser version at lines 3, 14, 122-123 and others.

What a sensible guard for Chromium 147 looks like:

1. Keep: `node --check`, the lexer-based scan infrastructure and the discovery self-test (these guard syntax errors and the vacuous-test case).
2. Change the target from "Chromium 69" to a named, configurable baseline. Two honest options: keep the conservative baseline on purpose and say so ("house style, not a browser limit"), or move the target to a conservative-but-real one such as ES2022 syntax (acorn `ecmaVersion: 2022`) and Chromium 120 level APIs, which leaves wide headroom under 147.
3. Keep rules that protect real constraints rather than versions: no `import` or `export` in classic scripts (a parse with `sourceType: 'script'` already catches this), no `type="module"` script tags unless the origin question in section 4 is resolved, no APIs missing from node (the dual-load requirement), and no new top-level globals beyond `window.TR` and `window.THING_CONFIG`.
4. Keep the CSS scan only for things that would break on the real target; drop the version-motivated bans.
5. Version numbers in messages should come from one constant at the top of the file so they cannot drift again.
6. The ground truth for "does this work on the device" is a device-side check, not a syntax list. The guard cannot replace it. See script C in section 8.

Because the guard is currently only a style fence, loosening it is a policy choice, not a bug fix. Not changed here.

## 7. Ranked recommendations

Quick wins (hours, no behaviour change):

1. Correct every doc and comment that says Chromium 68 or 69 (docs done in this change; code comments in 32 files, `style.css:44`, `index.html:69-71`, the toml and converter headers, and the guard comments are still open).
2. Fix the wrong explanation for the ES-module restriction (`vendor/README.md`, `index.html:66-67`): say `file://` opaque origin and CORS, mark as unverified until script A is run.
3. Run scripts A to C of section 8 once on the device and record the results in `docs/research/hardware-verification.md`.
4. Replace the margin-based tab spacing with flexbox `gap` (`style.css:44-45`), optional.

Medium:

5. Rework `syntax-guard.test.js` around a single named baseline constant, and decide whether the project keeps a deliberate conservative style or adopts a modern target (section 6).
6. Move to `@xterm/xterm` 6.0.0 and `@xterm/addon-fit` 0.11.0 with a hardware pass for scrolling and resize (the old package names are deprecated).
7. Measure the DOM renderer on the device, then trial the WebGL addon behind a config flag with fallback (unverified on `--use-angle=gles` with `--single-process`).

Not worth it:

8. Rewriting 501 `var` sites to `const` and `let`, arrow functions or template literals.
9. Converting to ES modules or adding a bundler (blocked or unproven on `file://`, breaks the node dual-load tests).
10. Adding `clamp()`, `aspect-ratio`, `inset`, container queries or `dvh` to a fixed 800x480 layout.

## 8. Exact checks to run on the device

All use the existing loopback devtools port. The device is reached over SSH; the port is bound to `127.0.0.1:9223`. Scripts A and B open a scratch target, so they change browser state while running and must be run by a person who accepts that; they are not run in this audit.

Script C (side-effect free, evaluates in the existing page via `Runtime.evaluate`, `returnByValue`):

```js
({
  ua: navigator.userAgent,
  gap: CSS.supports('gap', '4px'),
  inset: CSS.supports('inset', '0'),
  aspect: CSS.supports('aspect-ratio', '1 / 1'),
  clamp: CSS.supports('width', 'clamp(1px, 2vw, 3px)'),
  is: CSS.supports('selector(:is(a, b))'),
  optional: (() => { try { return new Function('a', 'return a?.b ?? 1')({}) === 1; } catch (e) { return String(e); } })(),
  cls: (() => { try { new Function('class A { x = 1; #y = 2; static { } }'); return true; } catch (e) { return String(e); } })(),
  webgl2: (() => { const c = document.createElement('canvas').getContext('webgl2'); return c ? c.getParameter(c.VERSION) : null; })(),
  ro: typeof ResizeObserver, sc: typeof structuredClone, intl: typeof Intl
})
```

Script A (module scripts over `file://`): through the browser websocket, `Target.createTarget` with `url: 'file:///usr/share/superbird-kiosk-default/test-module.html'` (a file with `<script type="module" src="m.js"></script>` and a trivial `m.js`, copied to the device by the person running the check), then read `Runtime.exceptionThrown` and `Log.entryAdded` for a CORS or `origin 'null'` message, then `Target.closeTarget`. Expected if the claim in section 4 is right: a blocked-by-CORS error. Result decides whether ES modules are possible without changing how the page is served.

Script B (WebGL under `--single-process`, `--use-angle=gles`): in a scratch target, `canvas.getContext('webgl2', {failIfMajorPerformanceCaveat: true})`, record the renderer string through `WEBGL_debug_renderer_info`, then load xterm 6.0.0 with `@xterm/addon-webgl` 0.19.0 and time a scripted flood of output against the DOM renderer, watching `chrome://gpu` data via `SystemInfo.getInfo`. Record heap with `Runtime.getHeapUsage`. Context loss must be exercised (`WEBGL_lose_context`) to confirm the fallback.

Also useful: `curl` against `/json/version` is already verified; extending it to `SystemInfo.getInfo` over the websocket gives the GPU and ANGLE backend actually in use.

## 9. Security note found during the audit

The browser runs as root with `--no-sandbox`, and exposes a remote debugging port on `127.0.0.1:9223` with `--remote-allow-origins=*`. Any local process, and any page that can reach loopback and satisfy the origin check (the flag allows every origin), can drive the browser. This is documented in `docs/hardware.md` as a known limitation, not fixed yet.

## 10. Older research files that still contain the 68/69 claim

`docs/research/architecture-proposals/frontend.md`, `button-knob-clock-design.md`, `hardware-verification.md`, `promotion-venues.md`, `research.md`, `settings-menu-feasibility.md`, `terminal-hardware-verification.md`, `webapp-feasibility.md`, `yocto-build-decisions.md`, `user-management-plan.md`, `user-management-plan-review.md`. These are historical records and were not edited.
