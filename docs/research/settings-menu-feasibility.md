# Settings/Menu Screen Feasibility (Bluetooth, brightness, terminal, clock)

Research date: 2026-11-16. Status: design only, no code written. Companion implementation ticket
should reference this file section-by-section, matching the convention of
[`button-knob-clock-design.md`](button-knob-clock-design.md) and
[`bt-peripheral-pairing-workflow.md`](bt-peripheral-pairing-workflow.md). **This revision
incorporates settled user decisions on the ESC/M button-role swap, the menu's navigation model,
and the BT-pairing-becomes-a-menu-item reconciliation — those are no longer open questions in this
document, they are specified design.**

---

## 1. Feasibility verdict

**Yes, with caveats.** Nothing here is blocked by hardware, firmware, or BlueZ build configuration
— the yocto-superbird BlueZ build already has full D-Bus/Adapter1 control (§4), the vendored
xterm.js 5.3.0 already exposes a live-settable `options` API for font/theme (§6), the read-only-
rootfs constraint already has an established `/var`-based persistence pattern (§8), and generic
Linux backlight sysfs control is a standard, well-documented kernel ABI (§5) — though the *exact*
node name on this specific device needs a live check this research pass could not perform (§5.1,
flagged, not assumed). The caveats are architectural, not technical blockers, and most of them are
now **settled** rather than open, per this revision:

- **Settled:** the menu is a dedicated overlay screen inside whichever app is currently the kiosk
  default, opened and closed by the same physical button acting as a toggle (§2, §3).
- **Settled:** BT pairing is one entry inside this menu, not a standalone chord trigger (§4, §10).
- **Settled:** `KEY_ESC` and `KEY_M`'s current job assignments swap (§3) — this is a real,
  explicitly user-directed change to two already-shipped, already-wired dispatch paths
  (`prototype/control-surface/agent/src/main.rs` and `terminal/frontend/app.js`), not a new
  mechanism.
- **Still open, flagged honestly:** the exact backlight sysfs node name for this hardware (§5.1)
  and whether an ambient-light auto-backlight daemon exists on `yocto-superbird` the way
  `sp-als-backlight` did on stock firmware (§5.2) — this research pass had no live device or BSP
  checkout access, unlike the sibling pairing-workflow doc's session, which did. Flagged as the
  top blocking unknown for the brightness entry specifically (§11 item 1).
- **Still open, inherited, not re-litigated:** the D-Bus dependency footprint question for
  `prototype/control-surface/agent` (`bt-peripheral-pairing-workflow.md` §4.2) — now shared by
  three features (pairing, BT power, and plausibly brightness/timezone per §9) rather than one,
  raising the stakes of deciding it but not changing the decision itself.

---

## 2. Navigation model — settled, not a tradeoff

**Specified design: a dedicated settings overlay, hosted inside `terminal/frontend` (the app
actually running full-time on the live device per `docs/development.md`), opened and closed by a
single physical-button toggle, navigated primarily by the knob and buttons, with touch as a
secondary input.**

### 2.1 Why an overlay inside the current app, not a separate page (reasoning retained from the
original pass, now stated as settled rather than weighed)

- **Single Chromium kiosk process, one default app at a time, no window manager.** Confirmed via
  `docs/development.md`/`docs/getting-started.md#swapping-which-app-is-the-default`: exactly one
  of `terminal/` or `prototype/dashboard/` loads into Chromium at boot, with no OS-level
  compositor/window-manager layer to bring up a cross-app settings surface the way a desktop or
  phone OS would. The menu has to be a DOM layer the host app's own JS shows/hides.
- **This repo's own precedent for "sustained mode, not a one-shot alert" is in-page state, not a
  page navigation** — `bt-peripheral-pairing-workflow.md` §5.2 already made and justified this call
  for the BT-pairing indicator; a settings menu is a bigger version of the same "stay where the
  user's attention already is" argument.
- **Google's Android-for-cars design guidance** independently corroborates the overlay-over-
  current-screen pattern for exactly this device category: *"The app bar in the media app template
  includes an option for a Settings control, which users can select to bring up an overlay with
  your app settings screen"* and *"Ideally, all settings should be accessible from a single screen,
  avoiding the use of complex dialogs"* (developers.google.com/cars/design, current doc).

### 2.2 The trigger: `KEY_ESC`, acting as a toggle (open when closed, close when open) — settled

Per the user's explicit decision: pressing `KEY_ESC` while the menu is **closed** opens it; pressing
it again while the menu is **open** closes it. No separate "exit"/"back-to-top" button or action is
designed — the single toggle covers both directions. **This supersedes `KEY_ESC`'s current job**
(hard-recovery/unwedge trigger, `button-knob-clock-design.md` §7) — see §3 for the full swap with
`KEY_M`, which takes over that job.

**Where the "is the menu currently open" state lives — settled: the frontend (`terminal/frontend/
app.js`), not `prototype/control-surface/agent`.**

Reasoning:
- `prototype/control-surface/agent` already broadcasts every raw `{"type":"button","code":1,...}`
  event for `KEY_ESC` unconditionally over the existing WS `ws://*:8765` channel (confirmed in
  `main.rs`'s `run_buttons()` — the `broadcast(&clients, &BridgeEvent::Button {...})` call happens
  for every key, every time, regardless of what else that key does). **No new agent-side logic is
  needed for the toggle itself** — the agent keeps doing exactly what it already does (relay the
  raw press), and the *only* agent-side change `KEY_ESC` needs is removing its hard-recovery
  `systemctl restart` side-effect (which moves to `KEY_M`, §3).
- The frontend is the only place that already knows whether the menu is currently showing (it's the
  thing rendering the menu's DOM), so it is the natural, lowest-friction owner of the one boolean
  (`state.menuOpen`) the toggle decision needs. Pushing this decision into the agent would mean the
  agent has to track UI state it has no other reason to know about, duplicating state that already,
  unavoidably, lives in the frontend anyway (the frontend has to know whether to render the overlay
  regardless of who decides whether to flip it) — a second source of truth with no benefit.
- This mirrors the existing, already-proven split in this codebase: `prototype/control-surface/
  agent` reports raw hardware facts; `terminal/frontend/app.js` already does 100% of the
  button-code-to-logical-action interpretation for every other key (`BUTTON_SESSION_MAP`,
  `scrollActiveScreen()`, etc., all client-side per `button-knob-clock-design.md` §1). `KEY_ESC`'s
  toggle logic is just one more entry in that same interpretation layer, not a new category of
  design.
- **Implementation shape (described, not coded):** `app.js`'s existing `'button'` handler in
  `connectBridge()`'s `onmessage` gains one more branch: on `msg.code === 1 && msg.value === 1`
  (press, not release/repeat — matching the existing ESC/KEY_M press-only convention already used
  for the hard-recovery branch), toggle `state.menuOpen` and call `renderMenu()`/`closeMenu()`
  accordingly. `KEY_ESC` remains absent from `BUTTON_SESSION_MAP` (it never was a session-switch
  key), same as today.

### 2.3 In-menu navigation semantics — settled, specified

With the menu open, physical-control semantics change from their normal (menu-closed) job to a
menu-local one, exactly the way `KEY_M`/`KEY_ENTER` already get selectively re-interpreted
depending on context in this codebase today (uinput passthrough vs. hard-recovery, per
`button-knob-clock-design.md`) — contextual remapping while a mode is active is an established
pattern here, not a new idea:

- **Knob rotation → move the highlighted row up/down** one entry per step (reusing the existing
  `'knob'` WS message and `KNOB_SCROLL_LINES_PER_STEP`-style step convention already wired for
  scrollback — same message, different client-side interpretation while `state.menuOpen` is true).
- **Knob push (`KEY_ENTER`, code 28) → select/confirm.** On a flat entry (e.g. "Bluetooth: ON" as a
  toggle row), this flips the toggle immediately. On a row that opens a value-editing sub-mode
  (brightness, font size, timezone — see §2.4), the first push *enters* that sub-mode (rotation now
  adjusts the value instead of moving the highlight) and a second push *confirms and exits* the
  sub-mode back to the row list. This is the same two-phase "enter a value" convention used by
  rotary-encoder+knob menu libraries generally (§2.4 cites a concrete, real one).
- **`KEY_1` → back/cancel, while the menu is open only.** Buttons 1-4 normally map directly to
  session switches (`BUTTON_SESSION_MAP`, `button-knob-clock-design.md` §1) — that mapping is
  meaningless while the menu has focus (there's no session to switch to), so `app.js`'s existing
  `'button'` handler gains a menu-open-specific branch: when `state.menuOpen` is true, `KEY_1`'s
  press is interpreted as "go back one level / cancel the current value edit" instead of "switch to
  session 0," and `KEY_2..KEY_4` are simply ignored while the menu is open (not repurposed further —
  kept simple deliberately, see §11 for the "assign 2-4 as category shortcuts" idea flagged as a
  future enhancement, not designed here). This choice (one dedicated back button, not all four)
  matches Google's cited "avoid complex dialogs, single-screen-where-possible" guidance by keeping
  the menu's own depth shallow (§2.4) rather than needing a rich multi-button navigation vocabulary.
- **Touch is secondary, confirmed available for free, not the primary design target.** The
  touchscreen already dispatches ordinary DOM tap events straight into whichever app is loaded with
  no bridge-daemon involvement (confirmed live, `docs/terminal.md`'s "Touchscreen tap/click focus
  fix" section) — so tapping a menu row to select it, or tapping a close affordance, works with zero
  additional wiring once the menu's DOM exists. This is an *additional* input path for the same
  actions the knob/buttons already drive, not a parallel design with its own layout — the menu's
  visual design (§2.4) does not need separate "touch mode" vs. "knob mode" layouts, since reverse-
  video row highlighting (the knob-driven affordance) and a tap target (the touch affordance) can
  be the same DOM element with the same CSS, just two different ways of reaching it.

### 2.4 Visual/interaction design, with cited precedent — not invented from scratch

This device's primary input for the menu (knob rotate + knob push, no pointer) is a materially
different interaction model from almost all phone/tablet settings-screen precedent, which assumes a
pointer/touch as primary. The right comparison set is **rotary-encoder-and-small-screen embedded
menus**, not touch-first dashboard precedent — confirmed by actually looking at both categories and
finding the touch-first category doesn't transfer:

**Real precedent actually found and used:**

- **Nest Learning Thermostat** (the single closest real-product analog: a round display, a rotating
  physical ring for navigation, a push-to-confirm interaction, no pointer): documented interaction
  is *"Turning the outer ring raises or lowers the temperature... Pushing the ring activates a menu
  of additional options"* and, for in-menu navigation specifically, Google's own support
  documentation confirms *"Rotate the dial clockwise until the Settings icon lights up... Press the
  thermostat toward the wall again to confirm your selection"* (cooperhewitt.org exhibition copy;
  support.google.com/googlehome, current Nest settings-navigation doc) — **rotate to move a
  highlight, push to confirm**, exactly the mechanism specified in §2.3, independently validated by
  a shipped, mass-market rotary-interface device with no touchscreen at all on its oldest models.
- **BMW iDrive** (automotive rotary controller, the most direct in-vehicle analog): *"Turn the
  Controller until the desired menu item is highlighted. Press the Controller"*
  (mycarusermanual.com's BMW iX iDrive guide, current) — the same rotate-to-highlight,
  press-to-confirm two-action vocabulary, confirming this isn't a Nest-specific quirk but a general
  convention for rotary-primary menu interaction in exactly this device class (in-vehicle small
  touchscreen + physical rotary control, the closest possible category match to this project).
- **A real, documented rotary-encoder-plus-small-screen menu *library*** (`mchobby/
  micropython-oled-menu`, a maintained open-source MicroPython menu system for an OLED screen +
  clickable rotary encoder — structurally the closest thing to "this project's exact input
  shape" found): documents exactly three entry *types*, which map directly onto this menu's three
  real value kinds:
  - **Label-only entries** (simple actions/navigation) — maps to this menu's toggle rows if reduced
    to two states, or to section headers.
  - **"Range" entries**: *"Turn the encoder right/left to change the value. Click the encoder to
    confirm the value."* — **this is exactly brightness and font-size's interaction** (§5, §6):
    entering the row starts a live-adjust sub-mode, rotation changes the value in place, a second
    click/push commits it. This document's §2.3 "value-editing sub-mode" design is this exact,
    already-shipped pattern, not invented.
  - **"Combo" entries**: a picker among a fixed list of named values, where *"The sign '>' indicates
    the current selection... Turn the button right/left to move the focus. Click the encoder button
    to confirm the value and exits combo"* — **this is exactly the timezone entry's interaction**
    (§7): a scrollable fixed list (IANA timezone names) is a Combo, not free text, since there is no
    keyboard assumed as the primary input here.
- **A from-scratch rotary-encoder-menu build log** (hackaday.io/project/184926-blexmang, a TTGO-
  T-Display project) documents the exact highlight-rendering mechanics worth citing for the visual
  design itself, not just the interaction model: *"To highlight an item in a menu, I draw a single-
  pixel rectangular outline of the whole line's area"* as one convention, with an **alternative,
  more common convention** confirmed by a second small-embedded-menu example
  (`alanesq/BasicOLEDMenu`, an Arduino OLED+rotary-encoder menu, source read directly): **reverse-
  video fill** on the highlighted row (`display.setTextColor(BLACK, WHITE)` on the selected line,
  normal `WHITE`-on-black elsewhere) — a solid color-swap on the whole row, not just an outline.
  **Recommendation: reverse-video fill, not an outline**, for this project specifically, because
  this repo already has an established "filled, high-contrast state indicator" visual language
  (the `.status-dot`/`.status-dot.ok` convention in `terminal/frontend/style.css`, cited in
  `bt-peripheral-pairing-workflow.md` §5.3) that a solid-fill row highlight matches more closely
  than a thin outline would, and a small 800×480 screen viewed from an arm's length in a car
  benefits more from a bold, unambiguous-at-a-glance highlight than a subtle one-pixel border.
- **Home Assistant wall-panel/kiosk-mode dashboards — explicitly checked, and explicitly excluded as
  primary inspiration, not just left out.** Real examples found (`WidowMaker/MiniPanel`,
  `dushyantgithub/simply-home`, Dwains Dashboard's wall-tablet mode) are uniformly **touch-first**:
  bottom nav bars, large tappable cards/tiles, grid layouts, long-press-and-hold gestures — every
  one of these assumes a finger as the primary pointer, with no rotary/knob input modeled at all
  (confirmed: none of the HA kiosk examples found mention a rotary encoder or non-touch primary
  input). This is the right precedent to rule out explicitly, not just skip past: it would actively
  mislead a grid/tile-based layout choice for *this* device, where touch is deliberately secondary
  per the user's own framing. **The one point of real transfer:** `dwainscheeren.github.io`'s
  wall-tablet-mode doc describes a long-press-to-reveal exit affordance for its own kiosk chrome —
  conceptually similar to this project's own "long-press/toggle a control to reveal a mode" pattern
  elsewhere (`bt-peripheral-pairing-workflow.md`'s chord design, now superseded per §4, and this
  doc's own `KEY_ESC` toggle), but that's a convergent interaction idea found independently in both
  places, not evidence that HA's touch-first *layout* conventions should be copied here.

**Resulting concrete design, citing which precedent each choice comes from:**

- **Flat, single-column list, not a grid.** Every rotary-primary precedent found (iDrive, Nest,
  both OLED-menu libraries) uses a vertical list, never a grid — grids assume 2D pointer movement a
  knob can't do directly (a knob is a 1D control), so a list is not a stylistic choice, it's the
  structurally correct match to a 1D rotation input, confirmed as the universal choice across every
  rotary-primary example found.
- **Top-level list of 4 entries** (Bluetooth, Brightness, Terminal, Clock/Timezone), each either a
  flat toggle row (Bluetooth power — see note below) or a row that opens a one-level-deeper
  sub-view (Bluetooth pairing's device list, Brightness's Range editor, Terminal's own two
  sub-settings, Timezone's Combo picker) — **maximum two levels deep**, directly matching Google's
  cited "all settings accessible from a single screen, avoid complex dialogs" guidance and
  `micropython-oled-menu`'s own stated scope ("One level with unlimited entries" per top menu,
  sub-views for Range/Combo editing).
- **Reverse-video fill for the highlighted row** (per `BasicOLEDMenu` precedent, reasoned above),
  reusing this project's existing `.status-dot`-style bold/filled visual language rather than a
  subtle outline.
- **Per-entry-type rendering convention** (direct application of `micropython-oled-menu`'s three
  types):
  - **Toggle** (Bluetooth power): row shows `Bluetooth` left-aligned, current state (`ON`/`OFF`)
    right-aligned — knob-push on this row flips it immediately, no sub-mode entry needed (a 2-value
    Combo collapsed to a single click-to-flip, the simplest case of the Combo pattern).
  - **Stepped/Range value** (brightness, font size): row shows the setting name and current value
    (`Brightness: 70%`, `Font size: 13`); knob-push enters a live-adjust sub-mode (row visually
    changes to show the value is "armed," e.g. the reverse-video highlight gains a blinking border
    or an explicit `[adjusting]` suffix) where further rotation changes the value live and a second
    knob-push commits and returns to the list — the exact `micropython-oled-menu` Range flow.
  - **Combo/fixed-list value** (timezone, font family, BT device picker during pairing): knob-push
    opens a scrollable sub-list of the fixed choices (timezone names, the vendored font stack's
    available families, discovered BT device names) with the same reverse-video-highlight-plus-
    knob-rotate convention as the top-level list, and a knob-push on a sub-list entry commits that
    choice and returns one level up — the exact `micropython-oled-menu` Combo flow, and the same
    shape `bt-peripheral-pairing-workflow.md` §4.3 already specified for its (now-superseded-as-a-
    standalone-trigger, but reused-as-UI, see §4) discovered-device list.
- **Touch affordance layered on top, not a separate design.** Each row/sub-list-entry is a normal
  DOM element with a tap handler calling the same action knob-push would call, and the whole
  overlay optionally gets a visible close affordance (e.g. a small "×" in a corner) for touch users
  who don't know about the `KEY_ESC` toggle — cosmetic, not required for a functional first cut,
  same "optional, skip unless requested" treatment this repo already gives other cosmetic additions
  (`button-knob-clock-design.md` §7, `bt-peripheral-pairing-workflow.md` §5.2).

### Sources
- `docs/development.md`, `docs/getting-started.md#swapping-which-app-is-the-default` — confirms one
  app loaded at a time, no window manager (read directly).
- `docs/terminal.md` ("Touchscreen tap/click focus fix") — confirms touch reaches the loaded app's
  DOM directly (read directly).
- `prototype/control-surface/agent/src/main.rs`'s `run_buttons()` (read directly) — confirms every
  key, including `KEY_ESC`, is already unconditionally broadcast raw over WS regardless of any
  special-cased side effect, the basis for §2.2's "no new agent-side toggle logic needed" claim.
- `terminal/frontend/app.js`'s `connectBridge()`/`BUTTON_SESSION_MAP` (read directly) — confirms the
  existing client-side-only interpretation-layer pattern this section's `KEY_ESC`/`KEY_1` handling
  extends.
- https://developers.google.com/cars/design/automotive-os/apps/media/create-your-app/design-settings
  — overlay-over-current-screen and single-screen-settings guidance (reused from the original pass).
- https://support.google.com/googlehome/answer/9244728 — Nest Learning Thermostat on-device settings
  navigation: rotate-to-highlight, press-to-confirm (quoted above).
- https://chsdmdev.wpengine.com/2014/11/12/drawings-for-nest-thermostat (Cooper Hewitt / Smithsonian
  Design Museum exhibition copy) — Nest's rotating-ring-activates-menu description (quoted above).
- https://www.mycarusermanual.com/bmw/ix/suv/2021-2025/controls--bmw-idrive — BMW iDrive rotate-to-
  highlight, press-to-select (quoted above).
- https://github.com/mchobby/micropython-oled-menu (README, read directly) — Label/Range/Combo
  entry-type taxonomy and their exact rotate/click semantics, the primary structural precedent for
  §2.3/§2.4's per-entry-type design.
- https://github.com/alanesq/BasicOLEDMenu (`oled.ino`, read directly) — reverse-video row-highlight
  convention (`setTextColor(BLACK, WHITE)` on the selected line).
- https://hackaday.io/project/184926-blexmang/log/205726-user-interface-elements — alternative
  outline-highlight convention and single-vs-double-click back/cancel vocabulary, read and compared
  against, not adopted as the primary highlight style (reasoned above).
- https://github.com/WidowMaker/MiniPanel, https://github.com/dushyantgithub/simply-home,
  https://dwainscheeren.github.io/dwains-dashboard-next/wall-tablet.html — Home Assistant wall-panel/
  kiosk-mode examples, checked and explicitly excluded as primary layout inspiration (reasoned
  above); the one point of convergent-but-independent interaction-idea overlap (long-press-to-reveal
  exit chrome) noted, not treated as transferable layout precedent.

---

## 3. The `KEY_ESC` / `KEY_M` role swap — settled, specified exactly

**Decision (user-directed, not a design choice weighed here): `KEY_ESC` (code 1) becomes the menu
open/close toggle (§2.2); `KEY_M` (code 50) takes over `KEY_ESC`'s current job as the hard-recovery/
unwedge trigger (`systemctl restart` of `terminal-agent`).** Both keycodes are already broadcast
unconditionally by `prototype/control-surface/agent` today (confirmed, `main.rs`'s `run_buttons()`)
— this is a dispatch-logic change in the two places that currently special-case one or the other,
not a new mechanism, exactly as the user framed it.

**Implementation status (2026-11-16): the `KEY_M`-side half of this swap is now IMPLEMENTED and
deployed live** (`prototype/control-surface/agent/src/main.rs`, see
[`button-knob-clock-design.md` §7](button-knob-clock-design.md#7-requirement-5-key_m-long-press-as-a-hard-unwedge-control-supersedes-the-original-key_esc-design-below)
for the shipped mechanism) — **with one refinement on top of §3.1's original proposal below**: the
user specified the restart should trigger on a **long-press** of `KEY_M` (≥1500ms, measured on
release) rather than on every press as `KEY_ESC` did, and — unlike §3.1's recommendation to drop
`KEY_M` from the uinput-replay allowlist entirely — the shipped version keeps `KEY_M`'s literal-`'m'`
keystroke passthrough completely unchanged on every press/release/repeat, short or long. A short tap
types `m` exactly as before; a long-press-and-release *also* fires the restart (so a long hold will
type one or more `m`s before the terminal session it was typed into gets torn down anyway — judged
an acceptable, harmless tradeoff rather than adding passthrough-suppression complexity for a
recovery control). The `KEY_ESC`-side half of this swap (the menu open/close toggle itself, §2.2) is
**not yet implemented** — that depends on the menu overlay UI existing in `terminal/frontend`, which
this document designs but which has not been built. `KEY_ESC` currently still broadcasts its raw
press/release over WS unchanged and does nothing else, same no-op state it had before this update.

### 3.1 Exactly what changes, file by file (described, not coded — this remains a research doc)

**`prototype/control-surface/agent/src/main.rs`:**
- The existing `if code == KEY_ESC && value == 1 { ...systemctl restart... }` branch (confirmed
  present, lines around the `KEY_ESC hard-recovery` comment) moves to `if code == KEY_M && value ==
  1`.
- The existing uinput-replay allowlist, today `{KEY_M, KEY_ENTER}` (confirmed in
  `build_uinput_passthrough()` and the `if code == KEY_M || code == KEY_ENTER` replay check), **must
  drop `KEY_M`** — once `KEY_M` is the hard-recovery trigger, it must stop also typing a literal `m`
  into the focused terminal session, for the same reason `KEY_1..KEY_4`/`KEY_ESC` are not replayed
  today (a key that triggers a system action should not *also* leak into whatever text field happens
  to have focus). The allowlist becomes `{KEY_ENTER}` only.
- `KEY_ESC` **gains no new uinput replay** — it was never replayed before (it's being repurposed as
  the menu toggle, a WS-message-only action per §2.2, not something that should ever type a literal
  Escape character into a shell either).
- The existing "sanity check… outside the documented/confirmed map" `eprintln!` condition (which
  lists `KEY_1..KEY_4`, `KEY_ESC`, `KEY_M`, `KEY_ENTER` as the expected set) needs no change — the
  set of valid codes is unchanged, only which one does what changes.

**`terminal/frontend/app.js`:**
- `KEY_M` (code 50) is currently unmentioned in `BUTTON_SESSION_MAP` and simply falls through to the
  normal keydown-replay path (typing `m`) — **this stays true for a normal, short press**; the swap
  only affects `KEY_ESC`, which gains the toggle logic described in §2.2. **`KEY_M`'s hard-recovery
  job lives entirely inside `prototype/control-surface/agent` (§3.1 above), exactly like
  `KEY_ESC`'s job did before the swap** — per `button-knob-clock-design.md` §7's own reasoning
  ("this is explicitly a *recovery* control... a design that routes this through app.js's JS event
  loop... is exactly the kind of path that can itself be wedged"), which applies identically to
  whichever key does this job. **No `app.js` change is needed for `KEY_M`'s new role at all** — only
  for `KEY_ESC`'s new toggle role (§2.2).
- One real UX tension worth flagging, not silently resolved: **`KEY_M` still needs to keep typing a
  literal `m` for a *short* press** (its original job, explicitly not given up — the task brief
  frames this as "M takes over ESC's old job," not "M stops being a letter key"), but must *also*
  trigger hard-recovery, a materially more destructive action, on... what, exactly? `KEY_ESC`'s old
  hard-recovery binding fired on a bare press (`value == 1`), with no hold-duration gate, because
  `KEY_ESC` had no other job to conflict with. `KEY_M` does have another job (typing `m`), so **this
  document recommends gating `KEY_M`'s hard-recovery action behind the same long-press/hold-
  threshold mechanism already designed and reasoned about for `KEY_ESC`'s *new* menu-toggle role
  being considered for `KEY_M` instead — i.e., swap which action is "bare press" and which is
  "long-press" isn't quite right either, since the menu toggle (§2) is specified as a bare press on
  `KEY_ESC`, not `KEY_M`.** Restated plainly: **recommend a short/long-press split on `KEY_M` itself**
  — short press (`value == 1` immediately released) keeps typing `m` as today (no uinput-replay
  change needed for this case beyond what §3.1 already removes... except §3.1 just removed `KEY_M`
  from the replay allowlist entirely, which would break this). **This is a real internal
  inconsistency in the literal instruction ("M takes over ESC's old job" vs. "M currently just types
  literal m") that this document surfaces rather than silently picks one side of** — see §11 item 2,
  ranked high because it affects `main.rs`'s exact dispatch logic, not merely cosmetic. The two
  resolutions available, neither designed further here:
  1. **Hold-threshold gate** (same hold-duration mechanism `button-knob-clock-design.md` already
     uses for nothing today, but the chord-detection pattern `bt-peripheral-pairing-workflow.md` §3
     designed for buttons 1+2 generalizes cleanly to a single key): short `KEY_M` press keeps typing
     `m` (uinput-replayed, as today); a long press (1.5-2s, matching the pairing-chord doc's own
     proposed threshold) triggers hard-recovery instead, and is *not* replayed. This preserves both
     jobs but adds complexity `KEY_ESC`'s old unconditional-press version didn't have.
  2. **Accept the loss of `KEY_M`-as-literal-m entirely**, matching this document's literal reading
     of "M takes over ESC's old job" as a full swap, not an additive one — `KEY_M` becomes
     exclusively the hard-recovery trigger, like `KEY_ESC` was, and a user who wants to type a literal
     `m` into a terminal session does so from a real/future BT keyboard (already the device's stated
     text-input story for anything beyond the four dedicated buttons) rather than the physical button
     labeled M.
  
  **This document does not pick one for the user — flagged as the one piece of the swap that needs
  an explicit answer before implementation, not quietly defaulted.**

### 3.2 Recommendation for `button-knob-clock-design.md`'s own key-mapping table

That document's key map (§0, §1, §7) states `KEY_ESC=1` is "the hard-recovery button" and `KEY_M=50`
"types the literal character 'm'" — **both of those sentences are now stale** per this swap.
**Recommend `button-knob-clock-design.md` get a short "superseded (2026-11-16): KEY_ESC and KEY_M's
roles were swapped, see `settings-menu-feasibility.md` §3" note at the top of its §7 and wherever
its key-map table/`Full key map on /dev/input/event0` summary appears** — this document does not
edit that file itself (out of scope, per this doc's own research-only remit), only recommends the
edit and specifies exactly what changed for whoever makes it.

### Sources
- `prototype/control-surface/agent/src/main.rs` (read directly, in full) — confirms the exact
  current `KEY_ESC` hard-recovery branch, the `{KEY_M, KEY_ENTER}` uinput allowlist, and the
  unconditional per-key broadcast this section's claims are based on.
- `terminal/frontend/app.js` (read directly, in full) — confirms `KEY_M`/`KEY_ENTER`'s current
  "intentionally absent from `BUTTON_SESSION_MAP`, arrives through normal keydown" treatment.
- `docs/research/button-knob-clock-design.md` §0, §1, §7 (read directly) — the key-mapping table and
  hard-recovery design this section's recommendation targets.
- `docs/research/bt-peripheral-pairing-workflow.md` §3 (read directly) — the chord/hold-duration
  mechanism this section's §3.1 resolution-option-1 proposes reusing for a single key.

---

## 4. Bluetooth: on/off, and pairing as a menu entry (supersedes the standalone chord trigger)

### 4.1 On/off mechanism — unchanged from the original research pass, retained in full

**Recommendation: `org.bluez.Adapter1.Powered`, set and observed directly over D-Bus — not
`rfkill`, not shelling out to `bluetoothctl`.**

| Mechanism | What it actually does | Appropriate for a programmatic kiosk daemon? |
|---|---|---|
| `rfkill block/unblock bluetooth` | Kernel-level radio soft-block, below `bluetoothd` entirely (`man7.org/linux/man-pages/man8/rfkill.8.html`). | **Not recommended as primary.** `org.bluez.Adapter`'s own doc (mankier.com/5/org.bluez.Adapter) documents a `PowerState` value `"off-blocked"` meaning *"Blocked by rfkill"* — BlueZ models rfkill as an external, lower, observable-but-not-driven-from-here layer; toggling it directly risks the degraded `off-blocked` intermediate state rather than a clean on/off. |
| `bluetoothctl power on/off` | Confirmed via its own man page: *"Uses: org.bluez.Adapter(5) property Powered"* (manpages.opensuse.org). | **Workable but not the best fit.** Same underlying mechanism as the row below, wrapped in a subprocess with no first-class subscribe path — a menu wanting instant, accurate state needs `PropertiesChanged` anyway. |
| `org.bluez.Adapter1.Powered`, set directly | The primitive the above two ultimately use. | **Recommended.** Same effective action as `bluetoothctl power`, no subprocess hop, free `PropertiesChanged` signal for reactive UI updates, and reuses the same `zbus` dependency `bt-peripheral-pairing-workflow.md` §4.2 already proposes for the pairing-agent work (§4.2 below) — one D-Bus dependency serving both BT features, not two. |

**Confidence: high**, cross-confirmed from `bluetoothctl`'s own man page and `org.bluez.Adapter`'s
own property doc, two independent primary BlueZ sources agreeing on the mechanism and its layering
relative to rfkill.

**Persistence caveat (feeds §8):** `org.bluez.Adapter`'s doc states `Powered` *"is not persistent.
After restart... it will reset back to false"* — confirmed directly, quoted in full in the original
pass. A stored on/off preference must be re-applied by whichever process owns this (§9) at its own
startup; BlueZ will not remember it.

**USB-CDC-NCM non-interference — confirmed, not assumed, unchanged from the original pass:**
`terminal/agent`'s vermelho link is a USB networking-gadget interface, architecturally separate from
the Bluetooth HCI controller `Adapter1.Powered`/`rfkill` act on; `rfkill(8)`'s own documented type
taxonomy (`wlan`/`bluetooth`/`uwb`/`wimax`/`wwan`/`gps`/`fm`/`nfc`) has no USB-networking type at
all, confirming the separation architecturally. **Confidence: high** by architecture; **not**
independently re-verified by actually toggling BT off against the live device (§11).

### 4.2 Pairing — now a menu entry, not a standalone button-chord trigger (settled)

**Position taken, per explicit user direction: BT pairing is one entry inside this settings menu's
Bluetooth section, with a live scanned-devices list rendered in the menu itself, and a visible
pairing-confirmation step — the buttons-1+2 long-press chord trigger designed in
`bt-peripheral-pairing-workflow.md` §3 is superseded, not kept as a parallel/secondary path.**

This is a firmer position than the original pass of this document took (which recommended *keeping*
the chord as a secondary "muscle memory" shortcut alongside a menu entry) — the user's direction is
explicit that the menu entry *replaces* the chord, not that both coexist. Recorded here as the
settled design, superseding this document's own earlier (pre-this-revision) reconciliation.

**What carries over from `bt-peripheral-pairing-workflow.md` unchanged, since only the *trigger*
is superseded, not the underlying BlueZ mechanics:**
- §2 (yocto-superbird BlueZ build confirmed to compile in HID-host support, no plugin restriction) —
  fully reusable, nothing about *how* pairing mode is entered changes what BlueZ itself supports.
- §4.1 (BlueZ agent API, `RequestConfirmation`/`DisplayPasskey`/etc., `KeyboardDisplay`/
  `DisplayYesNo` capability recommendation) — fully reusable; the agent-registration and
  confirmation-flow mechanics are independent of what UI element told the daemon to start.
- §4.3 steps 1b-1d, 2-5 (D-Bus calls to `SetDiscoverable`/`Pairable`/`StartDiscovery`, registering
  the agent, the `InterfacesAdded`-driven discovered-device surfacing, `Device1.Pair()`, the
  agent-callback relay) — fully reusable as the backend mechanics; only *where* the resulting
  discovered-device list and confirm/reject prompt render changes (into this menu's Bluetooth
  section, not a free-floating prompt).
- §5 (flashing red/blue status-bar indicator while pairing mode is active) — **retained as-is**,
  consistent with that doc's own reasoning (§5.2: device-wide background state belongs in the
  always-visible status bar regardless of what triggered it) — still correct regardless of whether
  a chord or a menu entry started the mode.

**What is explicitly superseded/removed:**
- `bt-peripheral-pairing-workflow.md` §3 in full (the buttons-1+2 chord/hold-duration detection
  design) — not needed. §4.3 step 1a (the chord firing `enter_pairing_mode()`) is replaced by: the
  menu's Bluetooth-section "Pair a device" row, on knob-push/tap, sends a new message over the
  existing WS channel (e.g. `{"type":"bt_start_pairing"}`) that calls the same `enter_pairing_mode()`
  function, just from a different trigger. **The function itself, and everything downstream of it,
  is unaffected** — this is a trigger swap, not a redesign of the pairing mechanics.
- §6 item 3 of that document (the chord's `gpio-keys-polled` autorepeat-wakeup open question) is
  moot — there is no chord to detect any more, so the single-key-press wakeup concern doesn't apply
  (a menu-entry knob-push is a discrete, already-reliably-detected event, not a hold-duration
  threshold needing a polling/wakeup strategy at all).

**Recommendation for `bt-peripheral-pairing-workflow.md` itself:** get a short **"Superseded
(2026-11-16): the buttons-1+2 chord trigger (§3) is retired; BT pairing is now reached via a menu
entry, see `settings-menu-feasibility.md` §4.2"** note added near the top of that document and at
the start of its own §3 and §6-item-3. This document does not edit that file (out of scope for a
research doc), only specifies exactly what the note should say and where.

### Sources
- https://man7.org/linux/man-pages/man8/rfkill.8.html, https://manpages.opensuse.org/Tumbleweed/
  bluez/bluetoothctl.1.en.html, https://www.mankier.com/5/org.bluez.Adapter — retained from the
  original research pass, unchanged conclusions.
- `docs/research/bt-peripheral-pairing-workflow.md` §2-§6 (read directly, in full, this revision) —
  the content explicitly reused (§2, §4.1, §4.3, §5) vs. superseded (§3, §6 item 3) above.

---

## 5. Screen brightness control mechanism

### 5.1 What was found, and what this research pass could not confirm live

**Stock firmware ran a dedicated daemon, `sp-als-backlight`** — confirmed directly in
`docs/research/hardware-verification.md`'s live `ps aux` capture: *"sp-als-backlight (ambient-light-
sensor-driven backlight daemon)"*, running alongside `bluetoothd`, `chromium-browser`, etc. on the
stock, pre-reflash unit. This is Spotify/DeskThing-firmware-specific vendor software — **there is no
evidence in this repo that an equivalent daemon exists, is built, or is enabled on `yocto-superbird`**
(unlike the BlueZ HID-host question, which `bt-peripheral-pairing-workflow.md`'s author had live
`ssh devel` access to the real BSP checkout to settle definitively, **this research pass had no such
access** — no BSP checkout, no live device, no `ssh devel` session available in this session).
**This is the single biggest unconfirmed item in this whole document, flagged honestly rather than
guessed around** (§11 item 1).

**What's confirmed instead, from general, well-documented, stable Linux kernel ABI (not
device-specific, but a reliable fallback target regardless of what `yocto-superbird` ships):**
- The Linux kernel's backlight subsystem exposes a standard sysfs interface at
  `/sys/class/backlight/<driver-name>/`, with `brightness` (read/write, the requested level),
  `actual_brightness` (read-only, what the hardware actually reports), and `max_brightness`
  (read-only, the top of the range) — confirmed directly from the kernel's own current
  documentation (`docs.kernel.org/gpu/backlight.html`): *"brightness R/W, set the requested
  brightness level... actual_brightness RO... max_brightness RO"*.
- Writing a value (0 to `max_brightness`) to `brightness` is the standard, driver-agnostic way to
  set backlight level — confirmed consistently across multiple independent real-world examples
  (ArchWiki's Backlight page: `echo 5 > /sys/class/backlight/acpi_video0/brightness`; a Stack
  Exchange example for an embedded PWM-backlight device:
  `echo 3 > /sys/class/backlight/backlight_lvds.0/brightness`) — the exact subdirectory name under
  `/sys/class/backlight/` is driver/platform-specific (`acpi_video0`, `intel_backlight`,
  `backlight_lvds.0` in the examples found; something SoC/panel-driver-specific on this device,
  unconfirmed), but the *mechanism* (a plain file write) is the same everywhere.
- **Permissions:** this sysfs attribute is `0644`, root-owned, by default, hardcoded in the kernel's
  own backlight core (`backlight.c`'s `DEVICE_ATTR_RW(brightness)`, cited via ArchWiki's own
  reference to the kernel source) — confirmed from two independent sources (ArchWiki, a Stack
  Exchange answer quoting the same hardcoded-permission fact) that a non-root process cannot write
  this file without either running as root or a udev-rule permission change. This matches the
  daemon-as-root model this project already uses elsewhere (`prototype/control-surface/agent`
  already needs root-equivalent capability for `EVIOCGRAB`/`uinput`), so no new privilege escalation
  is implied by this mechanism — just confirming the existing assumption holds here too.

### 5.2 Recommendation, with the open item stated plainly

**Recommended mechanism: a plain write to `/sys/class/backlight/<node>/brightness` from whichever
root-capable native process owns this setting (§9) — no subprocess, no D-Bus, simpler than the BT
case since this is a bare kernel sysfs file, not a service with its own IPC surface.**

**Two real open items, not glossed over:**
1. **The exact `<node>` name on this device/kernel build is unconfirmed.** Recommend checking
   `ls /sys/class/backlight/` on the live `yocto-superbird`-flashed unit (a 5-second check) before
   implementing anything here — this document deliberately does not guess a specific name, since
   every real-world example found uses a different one tied to its own specific panel/SoC driver.
2. **Whether an ambient-light auto-backlight daemon (an `sp-als-backlight` equivalent) exists and is
   enabled on `yocto-superbird` is unconfirmed, and matters for UX, not just mechanism.** If such a
   daemon exists and continuously re-writes `brightness` based on an ambient-light sensor reading,
   a menu-driven manual brightness slider would fight it (the daemon overwrites the user's chosen
   value on its own schedule) unless the daemon either (a) doesn't exist on this image at all (most
   likely, given it's not mentioned anywhere in this repo's `yocto-superbird`-era docs, unlike
   `bluetoothd`/`chromium`, which are), or (b) is disabled/stopped by whichever process applies a
   manual brightness choice, or (c) exposes its own "manual override" input this project should use
   instead of writing the raw sysfs node directly, bypassing the daemon's own logic. **Recommend
   checking the real `yocto-superbird` BSP checkout (the same `ssh devel` access
   `bt-peripheral-pairing-workflow.md`'s author had) for an ALS/backlight recipe or bbappend before
   implementing, the same way that document settled the BlueZ HID-host question — this document
   could not do that check itself in this research pass.**

### Sources
- `docs/research/hardware-verification.md` (read directly) — `sp-als-backlight` process confirmed
  running on stock firmware (quoted above).
- https://docs.kernel.org/gpu/backlight.html — kernel backlight-core sysfs ABI (`brightness`/
  `actual_brightness`/`max_brightness`), primary source, current kernel docs.
- https://wiki.archlinux.org/title/Backlight — real-world sysfs write example, default-permissions
  note citing the kernel's own hardcoded `0644` root-owned attribute.
- https://superuser.com/questions/1091697/setting-pwm-backlight-brightness-on-boot — a second,
  independent real embedded-device (`pwm-backlight`) example of the same write-to-sysfs mechanism,
  cross-confirming it's not an ACPI-only pattern.

---

## 6. Terminal settings scope (unchanged from the original research pass, retained in full)

### 6.1 Confirmed from the real vendored build (xterm.js 5.3.0, not "latest")

`setOption()`/`getOption()` do not exist in this vendored version — removed in xterm.js 5.0.0 per
the project's own GitHub release notes: *"The deprecated `getOption` and `setOption` APIs have been
removed in favor of `options`"*, migration `term.setOption('scrollback', 1000)` →
`term.options.scrollback = 1000`. The vendored 5.3.0 (`terminal/frontend/vendor/README.md`,
confirmed) post-dates this removal, so `term.options.<name> = value` / bulk-assign via
`term.options = {...}` is the correct, current API to design against.

**Confirmed live-adjustable without recreating the `Terminal` instance** (xterm.js's own current
docs, `xtermjs.org/docs/api/terminal/classes/terminal` + `.../interfaces/iterminaloptions`):
`fontSize`, `fontFamily`, `fontWeight`/`fontWeightBold`, `lineHeight`, `letterSpacing`, `theme`,
`cursorBlink`/`cursorStyle`, `scrollback`, and the rest of `ITerminalOptions` — all exposed through
the single `options` getter/setter. **One documented gotcha:** object-valued options
(`theme`) require a genuinely new object to trigger the update — *"a new object must be used in
order to take effect... `terminal.options.theme = {...newValue}` // This will work"* (xterm.js's own
docs, quoted directly) — a real implementation pitfall to flag, not designed around further here.

**Confidence: high** — primary source (xterm.js's own current API docs), cross-checked against the
exact 5.0.0 release note establishing `options` as the stable mechanism as of the vendored 5.3.0.

### 6.2 What this means for scope, concretely — **this is also exactly what the task's "font size,
font family/type" scope asks for, both confirmed live-adjustable**

- **Live-adjustable, no restart, no scrollback loss, zero `terminal/agent` (Rust) involvement:**
  font size and font family — both plain assignments on `app.js`'s existing module-level `term`
  instance (`const term = new Terminal({...})`, confirmed module-level, not per-session), exactly
  the two settings named in the task's concrete menu-entry list.
- **Needs a restart, real UX cost, explicitly out of this document's scope to design in full:** the
  session list (which shells/commands run in tabs 1-4) is read once at `terminal-agent` startup
  (`load_config()`, confirmed no live-reload path) — not part of the task's requested menu-entry
  list this revision, so not designed further here; flagged in §11 as before.

### Sources (unchanged from the original pass)
- http://xtermjs.org/docs/api/terminal/classes/terminal,
  http://xtermjs.org/docs/api/terminal/interfaces/iterminaloptions,
  https://github.com/xtermjs/xterm.js/releases (5.0.0 notes) — primary xterm.js API sources.
- `terminal/frontend/vendor/README.md`, `terminal/frontend/app.js`,
  `terminal/agent/src/main.rs`'s `load_config()` — read directly, confirming vendored version and
  current constructor/config-reload behavior.

---

## 7. Clock/timezone — manual timezone set, but NOT a manual "set time" control; reconciled against
the existing SSH-based clock-sync mechanism, with a position taken

### 7.1 The real mechanism, confirmed from `timedatectl`'s own documentation

`timedatectl`'s current upstream man page (`freedesktop.org/software/systemd/man/latest/
timedatectl.html`) confirms two independent, separately-invokable operations:
- **`set-time [TIME]`**: *"Set the system clock to the specified timestamp. This will also update
  the RTC time accordingly."* Requires **NTP sync to be disabled first** (`set-ntp false`) — multiple
  independent sources agree on this precondition, including a direct migration cheat-sheet
  (`linuxize.com/cheatsheet/timedatectl`: *"Disable NTP first when setting the clock manually... sudo
  timedatectl set-ntp false"* before `set-time`).
- **`set-timezone [TIMEZONE]`**: *"Set the system time zone... This call will alter the
  `/etc/localtime` symlink"* — confirmed directly from the same primary source. `list-timezones`
  enumerates the valid IANA names this call accepts, which is exactly the fixed-list "Combo" picker
  shape §2.4 already designed for this entry.
- **`set-ntp [BOOL]`**: controls whether `systemd-timesyncd` (or another detected NTP client) is
  active at all.

### 7.2 Reconciliation with the existing SSH-based clock-sync mechanism — position taken, not left
as a tension

**`terminal/agent`'s existing clock-sync thread (`button-knob-clock-design.md` §4, confirmed live in
`terminal/agent/src/main.rs`'s `clock_sync_loop()`/`probe_offset_ms()`) never touches the system
clock at all.** Confirmed by reading the code directly: `ClockState.offset_ms` is purely an
in-process, in-memory correction value, broadcast to the frontend as `{"type":"clock_sync",
"offset_ms":...}`, and `app.js`'s `renderClock()` computes the displayed time as
`new Date(Date.now() + state.clockOffsetMs)` — a client-side arithmetic adjustment on top of
whatever the browser's own `Date.now()` returns, never a write to `/etc/localtime`, never a call to
`date -s`/`timedatectl set-time`, and never persisted to disk (`ClockState` lives only in
`terminal-agent`'s process memory, reset to `Unsynced` on every restart, confirmed by reading
`main()`). This is deliberate and already well-reasoned in that design doc (an NTP-style
RTT-corrected offset probe against a trusted SSH host, chosen specifically *because* this device has
no general internet access for standard NTP — the whole reason a custom mechanism was built instead
of just using `systemd-timesyncd`/`timedatectl set-ntp true`).

**Position: do NOT add a manual "set system time" menu entry. DO add a manual "set timezone" entry.**
These are not symmetric, and treating them identically would be a mistake:

- **"Set time" would be redundant with, and strictly worse than, the existing mechanism, whenever
  it matters most (device connected to vermelho).** The existing offset-based display already shows
  a correct wall-clock without ever touching the system clock; a manual system-time-set control adds
  a second, competing way to influence what the clock widget shows, with no benefit in the connected
  case and only a brief, non-persistent benefit in the disconnected case — this device has **no RTC
  battery** (confirmed as a stated project constraint in the task brief and consistent with this
  being a low-cost embedded SoC board), so a manually-set system time does not survive a power cycle
  either, meaning a "set time" menu entry would only ever provide a temporary fix that both (a) the
  existing SSH-sync mechanism already provides automatically whenever vermelho is reachable, and (b)
  evaporates on the next reboot regardless, same as the SSH-sync's own in-memory-only state does.
  **Recommendation: leave this out of the menu entirely** — it adds a control surface for a problem
  the existing design already solves better, for the connected case, and only superficially for the
  disconnected case.
- **"Set timezone" is a genuinely different, non-redundant axis** — it governs *which civil-time
  rules* apply (DST transitions, UTC offset for display formatting) independent of *what the
  underlying instant in time is*, which the SSH-sync mechanism has no opinion on at all (it corrects
  an offset, not a zone). There's no existing mechanism in this repo that sets timezone, so this is
  a real gap the menu can usefully fill, with no conflict — confirmed by checking that neither
  `terminal/agent` nor `prototype/control-surface/agent` reference `/etc/localtime`,
  `timedatectl`, or any timezone concept anywhere in their source (read directly, both files, in
  full, this and prior research passes).

**One real persistence wrinkle, flagged, not glossed over:** `timedatectl set-timezone`'s mechanism
(rewriting the `/etc/localtime` symlink) targets a path under the root filesystem, which **is
read-only after boot** per this project's own already-established constraint
(`docs/dashboard.md`'s "a real on-device constraint drove the exact paths used here" section) — the
same constraint that pushed `noc-metrics.json` to `/var/thing-revived/` instead of `/usr/share`.
Unlike that file-write case, though, a `timedatectl set-timezone` call that only does a one-time
`mount -o remount,rw /` / symlink-rewrite / `mount -o remount,ro /` dance (the same pattern this
project's docs already use for pushing updates) **does not persist across a reboot** — the squashfs/
image's own baked-in `/etc/localtime` reverts the moment the running system's `rw` remount is gone,
since the underlying image itself was never rebuilt with the new symlink. **Recommendation: apply
the exact same pattern already designed for BT's `Powered`-is-not-persistent problem (§4.1,
§8) — store the chosen timezone in the `/var/lib/thing-revived/` settings file (confirmed writable
across reboot), and re-apply it via a fresh `timedatectl set-timezone <stored-value>` call (with its
own remount-rw/write/remount-ro dance) early at every boot**, rather than relying on the symlink
write itself to be durable. This is the same "OS state resets, so re-apply the stored preference at
every startup" shape as §4.1's BT-power persistence, not a new pattern.

**One secondary effect worth flagging for whoever implements this, not resolved here:** browser
engines commonly cache the process's effective timezone at startup rather than re-reading
`/etc/localtime` on every `Date` operation; whether Chromium 147 (confirmed the live on-device
version per `terminal/frontend/vendor/README.md`) picks up a `set-timezone` change without a
Chromium restart is **not verified in this research pass** — a real, cheap thing to check live
(change timezone, watch whether `new Date().getTimezoneOffset()` in a CDP `Runtime.evaluate` call
changes without restarting the kiosk process) before assuming a timezone change is visible
immediately. Flagged in §11.

### Sources
- https://www.freedesktop.org/software/systemd/man/latest/timedatectl.html — `set-time`/
  `set-timezone`/`set-ntp`/`list-timezones` primary doc, quoted above.
- https://linuxize.com/cheatsheet/timedatectl/ — `set-ntp false` precondition before `set-time`,
  corroborating source.
- `terminal/agent/src/main.rs`'s `clock_sync_loop()`, `probe_offset_ms()`, `ClockState` (read
  directly, in full) — confirms the existing mechanism never touches the system clock or persists
  to disk.
- `terminal/frontend/app.js`'s `renderClock()` (read directly) — confirms the purely client-side,
  `Date.now() + offset` display computation.
- `docs/dashboard.md` (read directly) — the read-only-root/writable-`/var`-family constraint reused
  for the `/etc/localtime`-persistence reasoning above.

---

## 8. Persistence design

### 8.1 Settings categories, what the OS persists on its own, and where a menu change needs storing

| Setting | Does the OS persist it on its own? | Where a menu-driven change needs storing |
|---|---|---|
| Bluetooth power state | **No** (§4.1: `Powered` resets to `false` on restart/unplug, confirmed from BlueZ's own doc). | Needs a stored preference, re-applied via `Adapter1.Powered` at whichever daemon's startup owns this (§9). |
| Bluetooth bonded/paired-device state | **Yes** — BlueZ's own `bluetoothd` writes bonded devices/link keys to `/var/lib/bluetooth/<adapter-mac>/<device-mac>/info` on its own; this project doesn't manage it. | Nothing to add — flagged for completeness, not a gap. **Not independently re-verified against this exact BSP's filesystem in this research pass** (general, stable BlueZ behavior, but worth a one-line `ls` check on real hardware, §11). |
| Screen brightness | Unconfirmed either way (§5.2) — depends on whether an ALS-auto-backlight-equivalent exists and whether it (or the kernel backlight core generically) persists the last written value across reboot on its own. Generic kernel backlight sysfs values are typically **not** persisted by the kernel itself (no backing store beyond the running kernel's in-memory device state) — **a desktop Linux convention worth noting, `systemd-backlight@.service`, exists specifically to save/restore this across reboots on systems that have it enabled**, but whether an equivalent exists/is enabled on this BSP is unconfirmed (§11). | Assume no OS-level persistence unless/until confirmed otherwise; store the chosen brightness in the same settings file as the other preferences, re-applied at startup by whichever process owns this (§9). |
| Terminal font size/family | **No** — pure in-memory `xterm.js` `term.options` state, no disk-backing today (confirmed, `app.js`). | Needs a settings file; `app.js` cannot write one directly from a `file://`-loaded page (no `fs.writeFile` equivalent in that sandbox), so persistence needs a small write path through `terminal/agent` (§9), which already has real filesystem access. |
| Timezone | **No, with a wrinkle** (§7.2): `timedatectl set-timezone`'s own symlink write targets a read-only-after-boot path, so even the "successful" write doesn't survive a reboot without re-applying it — functionally the same "OS state resets, re-apply stored preference at startup" shape as BT power, not a simpler case. | Needs a settings file, re-applied via a fresh `timedatectl set-timezone` call at every startup (§7.2), not relied upon to persist on its own. |
| Terminal session list (unrelated to this task's concrete entries, mentioned for completeness) | Already persisted — `terminal-agent-config.json`, read once at startup. | Already solved; out of this document's requested scope (§6.2). |

### 8.2 Where should the new settings file(s) live

Unchanged from the original pass: the project's own already-verified constraint (`docs/
dashboard.md`, confirmed via a real `adb reboot` test) is that `/var`, `/var/lib`, and `/home` stay
writable across reboots with no remount needed, while the rest of the root filesystem does not.
**Recommendation, extended to cover the fuller settings list now in scope:** a small JSON settings
file under `/var/lib/thing-revived/` — e.g. `/var/lib/thing-revived/settings.json` holding all four
menu-driven preferences (`bt_powered: bool`, `brightness: u8`, `terminal_font_size`/
`terminal_font_family`, `timezone: string`) as one small, infrequently-written file, rather than
four separate files — a single settings file is simpler to reason about for "what does this menu
currently show" than four independently-read files, and nothing about these four preferences is
large or high-frequency enough to need separate write-contention handling. (The original pass of
this document suggested two separate files for BT and terminal settings specifically to keep them
conceptually independent; with two more categories now in scope, one file with four top-level keys
is the more maintainable shape — a revision to that earlier recommendation, not a contradiction of
its reasoning, which was about not conflating *unrelated* concerns, not about file count for its own
sake.)

### Sources (unchanged from the original pass, extended reasoning only)
- `docs/dashboard.md` — read-only-rootfs / writable-`/var`-family constraint, confirmed via a real
  reboot test, reused here.
- `terminal/agent/terminal-agent-config.json`, `main.rs`'s `SSH_KEY_PATH` — existing
  `/var/lib/thing-revived/` precedent for this project's own persistent state.
- https://www.mankier.com/5/org.bluez.Adapter — `Powered` non-persistence, reused from §4.1.
- ArchWiki's Backlight page / `systemd-backlight@.service` general knowledge — cited in §5/§8.1 for
  the "desktop convention exists, this BSP's equivalent is unconfirmed" framing; **this specific
  claim (whether `systemd-backlight@.service` or an equivalent runs on `yocto-superbird`) was not
  independently checked against this project's actual BSP recipes in this research pass** — flagged,
  not asserted as checked.

---

## 9. Ownership / architecture recommendation

### 9.1 Who owns what — extended to cover the fuller menu-entry list

| Concern | Owning process | Why |
|---|---|---|
| Menu UI itself (overlay, highlight state, knob/button/touch handling) | `terminal/frontend/app.js` | It's the thing the user sees; no backend round-trip needed for pure UI state (§2). |
| BT power toggle + pairing (D-Bus `Adapter1.Powered`, agent registration, discovery) | `prototype/control-surface/agent` | Already the process with evdev access (for the menu's own physical-button handling, §2.3) and the one place `bt-peripheral-pairing-workflow.md` §4.2 already proposed adding a `zbus`-based D-Bus connection for pairing — reusing that same connection for the simpler `Powered` toggle avoids paying the dependency cost twice (unchanged recommendation from the original pass, now also covering §4's folded-in pairing UI backend). |
| Screen brightness (sysfs `brightness` write) | `prototype/control-surface/agent` | Same already-root-capable native process (§5.2's permissions note); a plain file write needs no new dependency at all, let alone a second daemon. |
| Timezone (`timedatectl set-timezone`, remount-rw dance) | `prototype/control-surface/agent` | Also needs root and a native process capable of running `timedatectl`/`mount`; grouping it with BT/brightness here means one process owns every "system-level control the menu can flip," consistent with the project's existing root-capable-daemon model, rather than splitting root-requiring actions across two different native processes for no isolation benefit. |
| Terminal font/family (live `term.options` writes) | `terminal/frontend/app.js` | Pure in-memory JS object mutation; no backend involvement needed for the live-adjust part at all (§6.2). |
| Settings persistence (reading/writing `/var/lib/thing-revived/settings.json`) | **Split, matching which process already has the live value**: `prototype/control-surface/agent` persists BT/brightness/timezone (it already applies them); `terminal/agent` persists font/family (the frontend can't write files itself, and `terminal/agent` is the native process already in that WS relationship, `ws://*:8766`) | Avoids inventing a reason for one of these two already-existing native processes to reach into the other's WS connection just to perform a file write; each owns persistence for the settings it already applies. |

### 9.2 Net recommendation — restated, not revised, from the original pass

- **Not** a new dedicated settings daemon. This repo's "one process per feature" instinct
  (`terminal-agent`, `control-surface-agent`, `noc-agent`) is for things needing their own always-on
  OS-level resource; a settings menu is a UI layer over *existing* ownership domains, not a new
  resource of its own. The fuller entry list in this revision (brightness, timezone) still fits
  cleanly into the same two existing processes (§9.1 table) — nothing about the expanded scope
  changes this conclusion.
- The menu UI talks to **both** existing WS connections depending on which setting is being
  changed — `ws://*:8765` (`control-surface-agent`) for BT/pairing/brightness/timezone,
  `ws://*:8766` (`terminal-agent`) for terminal font/family persistence — exactly mirroring how
  `app.js` already splits session-chrome vs. PTY I/O across the same two connections today.

### Sources
- `docs/research/bt-peripheral-pairing-workflow.md` §4.2 — D-Bus-dependency scope-increase flag,
  reused.
- `terminal/agent/src/main.rs`, `prototype/control-surface/agent/src/main.rs` (read directly) —
  confirm the existing "native agent does what a browser sandbox cannot" pattern this extends.

---

## 10. Reconciliation with `bt-peripheral-pairing-workflow.md` — restated as settled

See §4.2 in full for the detailed mechanics. **Summary position: BT pairing is folded entirely into
this settings menu as a Bluetooth-section entry with a live discovered-device list and a visible
confirmation step; the standalone buttons-1+2 long-press chord trigger from that document's §3 is
retired, not kept as a parallel path.** Everything else in that document (the BlueZ build-
configuration findings, the BlueZ agent-API mechanics, the discovered-device D-Bus plumbing, and the
flashing status-bar indicator) carries over unchanged — only the *trigger* that starts pairing mode
changes, from a physical-button chord to a menu-row action. Recommend that document receive a short
superseded-note pointing here (§4.2 specifies exactly what it should say); this document does not
edit that file itself.

---

## 11. Open questions, ranked by how blocking each is

1. **Backlight sysfs node name and whether an ALS-auto-backlight daemon exists/is enabled on
   `yocto-superbird` (§5.1, §5.2) — the single biggest unconfirmed item in this revision.** Unlike
   the BT HID-host question (which `bt-peripheral-pairing-workflow.md`'s author resolved with live
   `ssh devel` BSP-checkout access), this research pass had no device or BSP access at all. Needs a
   live check (`ls /sys/class/backlight/`, and a BSP recipe/bbappend search for an ALS/backlight
   service) before the brightness entry can be implemented with confidence, not just a reasonable
   guess.
2. **The `KEY_M` short-press-vs-hard-recovery conflict (§3.1) is a real, unresolved internal
   tension in the literal swap instruction, not a minor implementation detail.** Does `KEY_M` keep
   typing a literal `m` on a short press (needing a new hold-threshold gate before hard-recovery
   fires) or does it give up that job entirely (a simpler, fully-literal reading of "M takes over
   ESC's old job")? This needs an explicit decision before `main.rs`'s dispatch logic can be written
   — flagged as a question for the user/Thinker, not defaulted either way in this document.
3. **D-Bus dependency footprint in `prototype/control-surface/agent` (§9.1) — now shared by three
   features (pairing, BT power, and plausibly timezone/brightness if they also route through that
   process's D-Bus/shell-out capability) rather than one or two.** Still the single biggest
   architecture fork inherited from the sibling doc (`bt-peripheral-pairing-workflow.md` §4.2): grow
   the always-on binary, or split a short-lived sidecar. Ranked below items 1-2 here because it's a
   known, already-well-articulated tradeoff rather than a fresh unknown, but the stakes of getting it
   right have gone up with more features depending on the same decision.
4. **Whether Chromium picks up a `timedatectl set-timezone` change without a kiosk-process restart
   (§7.2)** — a cheap, concrete live check (CDP `Runtime.evaluate` of `new Date().
   getTimezoneOffset()` before/after a timezone change, no restart in between), not yet performed.
5. **BlueZ bonded-device storage location (`/var/lib/bluetooth/...`, §8.1) was asserted from general,
   stable BlueZ knowledge, not re-confirmed against this exact BSP's filesystem in this research
   pass.** Low risk (extremely standard, long-stable BlueZ behavior), worth a one-line `ls` check
   before treating it as load-bearing for any future "forget this device" menu feature.
6. **USB-CDC-NCM/Bluetooth non-interference (§4.1) remains an architectural inference, not a live
   test** — clear-cut and well-sourced, but a 30-second live check (toggle BT off, confirm the SSH
   link to vermelho survives) is cheap and worth doing before shipping, not a design risk worth more
   research time now.
7. **Exact hold-duration threshold, if §3.1's option 1 (hold-gate) is chosen for `KEY_M`** —
   inherits the same "reasonable engineering judgment, not measured" caveat `button-knob-clock-
   design.md`/`bt-peripheral-pairing-workflow.md` already flagged for their own threshold constants.
   Low risk, low priority.
8. **Button-2-4 repurposing as menu category shortcuts (§2.3), deliberately not designed here** — an
   optional future enhancement flagged, not a gap in the current design (`KEY_1`-as-back plus knob
   navigation is a complete, if slightly slower, navigation scheme on its own).
9. **Pixel-level visual polish beyond the layout/highlight/entry-type conventions specified in §2.4**
   (exact colors beyond "reuse the existing status-dot palette," spacing, font choice for the menu
   chrome itself) is intentionally left to whoever implements this, consistent with this document's
   own remit (feasibility/architecture research, not a pixel-perfect mockup).

---

## 12. UI revision: ESC confirm/long-press-return, input isolation, contextual hint bar

Research date: 2026-11-16. Scope: three user-requested changes to the already-shipped settings
menu (`terminal/frontend/app.js`, `style.css`, commits `f64a01a`/`b4ae616`, read directly in full
for this revision). **Design only, no code written** — same remit as the rest of this document.

### 12.1 `KEY_ESC` as confirm (tap) / return (hold) — real precedent, concrete threshold

**The dual-role pattern itself is real and documented, not an invented convenience.** Checked three
independent sources, two primary:

- **Apple TV Siri Remote's Back button, current Apple Support documentation (primary, live doc,
  checked 2026-11-16):** *"While browsing content, press the Back button one time to return to the
  previous screen. Or press and hold the Back button to go to the Home Screen."*
  (support.apple.com/en-us/102337). This is the closest real single-button analog to what's being
  asked here — **tap = one-level-back/local action, hold = jump-to-top** — mapped onto Apple's
  "Back" button rather than a "confirm" button, but structurally identical to this project's
  "tap = confirm (a local, forward-moving action), hold = back/up-a-level" request: both use hold
  duration, not a second button, to pick between a local action and an escape-upward action.
  **Confidence: high** (primary, current, direct quote).
- **Android TV's own developer guidance on back-button behavior** (developer.android.com/
  training/tv/get-started/navigation, primary, current): mandates *"predictable back button
  behavior"* — each press goes to the logical previous destination, repeated presses eventually
  reach the app root and then exit — the same "back climbs one level at a time, not a single jump"
  shape already specified for this menu's `KEY_1`-as-back in §2.3. This source documents the
  *tap*-back semantics precisely but does **not** itself document a press-and-hold-for-home
  convention on Android TV remotes; that comes from a second, weaker source.
- **Android TV/Shield remote long-press-back-for-home**, corroborated only by community/forum
  sources (Emby community thread, Button Mapper demo video) describing shipped Shield-remote
  behavior, not a Google first-party doc. **Confidence: medium** (real, consistently described
  behavior across independent forum threads, but not confirmed against a Google primary source in
  this pass) — included only as a second real-world data point alongside Apple TV's, not as the
  primary citation.

**Recommendation: implement this as tap vs. hold on `KEY_ESC` while the menu is open, exactly as
the user specified, with a `450ms` threshold — deliberately much shorter than this project's
existing `1500ms` `KEY_M` hard-recovery threshold, and for a reasoned, cited reason, not just "feels
right":**

- **This project's own existing `1500ms` `KEY_M` threshold is intentionally slow** — `button-knob-
  clock-design.md` §7's own stated reasoning (quoted in this doc's §3) is that hard-recovery is
  deliberately hard to trigger by accident, the opposite design goal from a routine per-row
  confirm/back action a user will hit dozens of times per menu session.
- **Primary source for a much shorter "has this become a hold yet" threshold: Android's own
  platform default.** AOSP's `ViewConfiguration.java` (android.googlesource.com/platform/
  frameworks/base, read directly) defines `DEFAULT_LONG_PRESS_TIMEOUT` as the system-wide constant
  answering exactly this question ("how long before a press turns into a long press") for every
  Android app and the OS's own UI — **currently `400ms`** in the live `master` branch, and `500ms`
  in an older tagged revision checked for comparison (`22c055e`) — confirming this constant has
  only ever lived in the 400-500ms band across Android's own history, not drifted toward anything
  near 1500ms. A second, independent real-world example (a Power Apps tap-vs-long-press
  implementation guide, powertower.dev, current) uses `500ms` as its own chosen threshold for the
  identical tap/hold-disambiguation problem, converging on the same order of magnitude from a
  completely unrelated platform. **Confidence: high** — primary OS-vendor source plus one
  independent corroborating implementation, same number band.
- **Recommended value: `450ms`**, inside the 400-500ms band both sources above converge on, and
  comfortably faster than the `1500ms` recovery trigger so the two don't feel like the same kind of
  "hold" gesture to the user. Not independently user-tested on this specific hardware's knob/button
  feel — flagged as a reasonable-engineering-judgment number, same caveat this document's §11 item
  7 already attaches to the `KEY_M` threshold.

**Implementation shape (described, not coded), grounded in the real current `app.js`:**

- `handleButton()` (confirmed current code, `app.js` lines ~292-315) currently acts only on
  `value === 1` (press) and returns immediately on anything else. This has to change for `KEY_ESC`
  specifically while the menu is open: evdev's own value convention (0 = release, 1 = press,
  2 = autorepeat, already relied on elsewhere in this codebase per `button-knob-clock-design.md`)
  gives `handleButton` everything it needs — it already receives every value, it just currently
  discards non-`1` values up front.
- **Recommended dispatch split, by `state.menuOpen`:**
  - **Menu closed:** unchanged — `KEY_ESC` press (`value === 1`) calls `toggleMenu()` (→`openMenu()`)
    immediately, exactly as today. No hold-detection needed on the opening edge; the user's own
    framing ("when the menu is CLOSED, a press still opens it") only asks for the single-action
    open, not a dual-role.
  - **Menu open:** on `KEY_ESC` press (`value === 1`), record `state.menu.escPressedAt =
    Date.now()` and return (no action yet — this mirrors why `KEY_M`'s hard-recovery branch needs a
    press/hold/release lifecycle, same shape, shorter number). On `KEY_ESC` release (`value === 0`),
    compute `Date.now() - state.menu.escPressedAt`: **under `450ms` → confirm** (call the same
    `menuSelect()` knob-push already calls, taking over that job per the user's request); **at or
    over `450ms` → return** (call `menuBack()` if `state.menu.stack.length > 1` or
    `state.menu.adjusting`, else `closeMenu()` if already at the top level — exactly the "go up one
    level, or close if already at the top" behavior the user specified). Autorepeat (`value === 2`)
    while held is ignored (no action needed mid-hold, only at release, same as not needing a visual
    "held" indicator for a first cut — optional polish, not required).
  - **Knob-push (`KEY_ENTER`) keeps its current `menuSelect()` role unchanged** — the user's request
    is "ESC becomes confirm too," not "knob-push stops being confirm." Both now call `menuSelect()`,
    which is a no-op conflict risk only if both fire in the same tick, which they structurally can't
    (one is a button code, one is a knob-encoder code, delivered as separate WS messages per
    `bt-peripheral-pairing-workflow.md`/`button-knob-clock-design.md`'s already-confirmed protocol).

### 12.2 Input isolation while the menu is open — the real gap is narrower than assumed, found by direct code reading

**Checked directly against the live `app.js` (not assumed from the task brief's description), and
the picture is better than the brief states for two of the three input paths, with one real,
specific gap identified:**

- **Buttons 1-4 (bridge `'button'` messages) are already fully isolated.** `handleButton()`
  (confirmed, lines ~292-315) checks `if (state.menuOpen) { ...; return; }` **before** ever reaching
  the `BUTTON_SESSION_MAP` lookup/`switchToSession()` call below it — a button-code press while the
  menu is open cannot reach `switchToSession()` at all, structurally, not just by convention. **No
  gap here.**
- **The knob is already fully isolated.** `connectBridge()`'s `onmessage` handler (confirmed, lines
  ~278-283) branches on `state.menuOpen` *before* calling either `menuKnob(msg.delta)` or
  `scrollActiveScreen(msg.delta)` — a knob-rotate message while the menu is open never reaches
  `scrollActiveScreen()`/`term.scrollLines()` at all. **No gap here either.**
- **The real, specific gap: `onKeyDown()` (confirmed, `app.js` lines ~834-851) has no
  `state.menuOpen` check at all.** This function is reached two ways, both still live while the
  menu is open: (a) `control-surface/agent`'s uinput passthrough replaying `KEY_ENTER` as a literal
  synthetic `Enter` keydown on whatever has DOM focus (confirmed in this doc's §3.1's description of
  the `{KEY_ENTER}` replay allowlist — this is a kernel-level synthetic event, not something
  `app.js`'s own `state.menuOpen` branching in `handleButton()`/`connectBridge()` can intercept,
  since it never arrives as a bridge WS message at all), and (b) any real/future BT keyboard
  keystroke (per this doc's own §4.2-adjacent sibling research on BT-HID, cited in `app.js`'s own
  comment above `KEY_SEQUENCES`). Both paths land in `onKeyDown()`, which unconditionally calls
  `sendAgent({type:'input', session: state.activeSession, data: data})` — **meaning a knob-push
  while the menu is open today both correctly calls `menuSelect()` (via the already-isolated
  `handleButton`/`KEY_ENTER`... actually via the bridge `'button'` path for `KEY_ENTER`, confirmed
  handled inside the `if (state.menuOpen)` branch) AND simultaneously types a literal `\r` into the
  active terminal session's PTY**, because the *same physical knob-push* is delivered to the
  frontend twice by design (once as a bridge WS `'button'` message per `handleButton`, once as a
  uinput-replayed DOM keydown per `onKeyDown`) and only the first of those two delivery paths is
  gated by `state.menuOpen`.
- **Exact fix location: `onKeyDown(evt)`, `app.js` line 834, first line of the function body.**
  Add `if (state.menuOpen) { evt.preventDefault(); return; }` before the existing `Ctrl+A..Z`/
  `KEY_SEQUENCES`/single-char branches. This is the single, precise, minimal gate point: it sits
  upstream of every existing branch in the function, so no individual key-mapping branch needs its
  own menu-aware special case, and it preserves `onKeyDown`'s `{capture: true}` listener registration
  (confirmed, `main()`, line ~882) unchanged — the gate is inside the handler, not a change to how/
  when the handler is attached. `evt.preventDefault()` is kept on the early-return path so a focused
  `#screen` (which still has DOM focus while the menu overlay is showing, since nothing in the
  current code moves focus onto the menu overlay's own DOM) doesn't fall through to any browser
  default either.

**Confidence: high on all three findings** — each is a direct, line-by-line reading of the real
current `app.js`, not an inference from the task brief's description (which was accurate for
buttons/knob being a *plausible* risk to check, but the actual code already closes two of the three
paths; the third, `onKeyDown`, is the one genuinely open gap, and now has an exact fix location).

### 12.3 Contextual hint bar — real precedent checked, one is a dead end, one substitutes

**Nocturne and DeskThing, checked directly (GitHub READMEs, marketing site, Thing Labs community
wiki FAQ with real UI screenshots) — neither shows a persistent physical-button legend/hint bar on
the Car Thing's own screen, and there's a structural reason why, not just an oversight:**

- **Nocturne (`usenocturne/nocturne`, `usenocturne/nocturne-ui`; usenocturne.com marketing site;
  Ars Technica/Trend Hunter coverage, all checked):** its on-device UI (`packages/ui/`, "React 19 +
  Vite kiosk app") is a touch-first, Spotify-style full-screen album-art/playback interface — the
  marketing site's own screenshot gallery and feature list ("full playback controls, playlist
  management, artist and album exploration... gesture controls") describe tap/swipe interaction
  throughout, not a knob-primary input model. **No persistent control-legend bar found or
  referenced anywhere in the checked sources.** This matches `settings-menu-feasibility.md` §2.4's
  own already-reasoned conclusion about touch-first dashboards generally (HA kiosk examples): a
  legend bar explaining *hardware button* roles has no reason to exist in a UI whose primary input
  is direct on-screen tapping, where the thing being tapped already *is* the affordance.
- **DeskThing (`ItsRiprod/DeskThing`; deskthing.app; Thing Labs community wiki FAQ, checked
  directly, including real screenshots of its settings flow):** settings/configuration (brightness,
  RNDIS, playback location) are driven from a **separate desktop companion app** running on a PC,
  not from on-device button presses interpreted by a legend on the Car Thing's own screen at all
  (confirmed directly from the FAQ's own screenshots: "backlight brightness" is adjusted via a
  slider in the desktop app's "Device Details" panel, not a Car-Thing-side menu). **No on-device
  hint bar exists because there's no on-device button-driven settings UI to need one for** —
  structurally a non-case, not evidence against the pattern.
- **Neither project is the right comparable for this specific element, found by actually checking
  and ruling both out, same as §2.4's already-established practice of excluding a close-but-wrong
  precedent rather than silently skipping it.** The right substitute, found instead:
  **automotive rotary-controller infotainment systems with a persistent, context-sensitive bottom
  action bar — BMW iDrive specifically.** Checked video-tutorial transcripts and a live Reddit
  thread about the feature breaking after an update (r/AndroidAuto, "BMW iDrive bottom bar controls
  disappears after OneUI 9..."), which independently confirms, as a side effect of being a bug
  report about its *absence*, that iDrive screens normally carry **a persistent bottom bar of
  context-sensitive control labels** (the exact "OK"/"BACK"-style row this doc's own §2.4 already
  cited BMW iDrive for its rotate-to-highlight/press-to-confirm *mechanism* — this is the same
  real product's *hint-bar* convention, not a different product). **Confidence: medium** — the
  Reddit source confirms the bar's existence indirectly (as something users notice is now broken/
  missing) rather than a primary BMW documentation page describing it directly; no primary BMW UX
  doc for the bottom bar specifically was found in this pass, only corroborating user reports.
- **This project's own shipped code is already most of the way there.** `renderMenu()` (confirmed,
  `app.js` lines ~805-810) already drives `#menu-hint` contextually, today distinguishing exactly
  two states by `state.menu.stack.length`. This section extends that existing mechanism, not a new
  one — same element (`.menu-hint` in `style.css`, lines 235-241, already styled per the project's
  dark/monospace/`--text-dim` language), same `renderMenu()` call site, more branches.

**Recommendation: extend the existing `#menu-hint` logic in `renderMenu()` (lines 805-810) to branch
on four states instead of two, reflecting the new ESC semantics from §12.1:**

| Context (how to detect, using existing state) | Exact hint text |
|---|---|
| Top-level list (`state.menu.stack.length === 1`, `!state.menu.adjusting`, not `bt-confirm`) | `knob: move · esc: confirm · esc (hold): close` |
| Any non-root list/nav screen (`state.menu.stack.length > 1`, `!state.menu.adjusting`, `currentScreenId() !== 'bt-confirm'`) — covers Bluetooth list, "Pair a device" discovery list, Terminal list, timezone-groups, timezone-zones | `knob: move · esc: confirm · esc (hold): back` |
| A range entry armed for live-adjust (`state.menu.adjusting === true`) — brightness, font size | `knob: adjust · esc: confirm · esc (hold): cancel` |
| BT pairing confirm/deny prompt (`currentScreenId() === 'bt-confirm'`) | `knob: move · esc: confirm selection · esc (hold): cancel pairing` |

- **Wording rationale:** every line keeps the existing `knob: ...` clause's role unchanged (still
  governs movement/adjustment, per §2.3, untouched by this revision) and only the `esc:`/`esc
  (hold):` clauses change per §12.1's new dual role — `esc: confirm` is identical wording across all
  four rows (the *action* ESC-tap performs is always "confirm the current thing," never relabeled
  per screen, keeping the legend itself simple and memorizable) while `esc (hold):` is the one
  clause that legitimately differs by context, because "what hold does" genuinely differs (close
  the whole overlay vs. step up one level vs. abandon an in-progress value edit vs. decline a
  pairing) — exactly the "contextual, not static" requirement from the task brief, done with the
  minimum necessary number of distinct strings (four, not one per screen).
- **`KEY_1`-as-back (§2.3, already shipped) is unaffected by and not mentioned in the new hint text**
  — out of scope for this revision; if the project later wants `KEY_1` reconciled with ESC's new
  hold-to-back role (two different buttons now both meaning "back" in some contexts) that's a
  follow-up design question, not resolved here, flagged for whoever implements §12.1/§12.3 to raise
  with the user if it feels redundant in practice.
- **Implementation shape:** replace the existing two-branch ternary at `renderMenu()` lines 805-810
  with a small helper (e.g. `menuHintText()`) checking, in order: `state.menu.adjusting` →
  row 3 text; `currentScreenId() === 'bt-confirm'` → row 4 text; `state.menu.stack.length > 1` →
  row 2 text; else → row 1 text. No new state is needed — every condition above already exists on
  `state.menu` today.

### Sources
- https://support.apple.com/en-us/102337 — Apple TV Siri Remote Back-button tap/hold convention,
  primary, current, quoted in full above (§12.1).
- https://developer.android.com/training/tv/get-started/navigation — Android TV predictable-back-
  button guidance, primary, current (§12.1).
- https://emby.media/community/topic/105206-home-page-button,
  https://www.youtube.com/watch?v=86dabiwcrXI (Button Mapper demo) — community-sourced corroboration
  of Shield/Android-TV-remote long-press-back-for-home; medium confidence, not a Google primary
  source (§12.1).
- https://android.googlesource.com/platform/frameworks/base/+/master/core/java/android/view/
  ViewConfiguration.java and the `22c055e` tagged revision of the same file — AOSP's own
  `DEFAULT_LONG_PRESS_TIMEOUT` constant, `400ms` current / `500ms` historical, primary source for
  the recommended `450ms` threshold (§12.1).
- https://powertower.dev/2025/10/23/tap-vs-long-press-in-power-apps-measure-button-press-duration-
  and-trigger-different-actions — independent `500ms` tap/long-press threshold example, corroborating
  the same number band from an unrelated platform (§12.1).
- `terminal/frontend/app.js` (read directly, in full, this revision) — `handleButton()` (lines
  ~292-315), `connectBridge()`'s `onmessage` (lines ~243-290), `onKeyDown()` (lines ~834-851),
  `renderMenu()` (lines ~760-811), `main()` (lines ~868-893) — the exact current code every claim
  and code-location pointer in §12.1-§12.3 is checked directly against, not assumed.
- `terminal/frontend/style.css` (read directly, in full) — `.menu-hint` (lines 235-241), confirming
  the existing element/styling this section's hint-bar recommendation reuses rather than replaces.
- https://github.com/usenocturne/nocturne, https://github.com/usenocturne/nocturne-ui,
  https://usenocturne.com — checked directly for a persistent button-legend convention; none found,
  reasoned why (touch-first UI) in §12.3.
- https://github.com/ItsRiprod/DeskThing, https://deskthing.app,
  https://carthing.wiki/thinglabs-apps/deskthing/faq — checked directly (including real screenshots
  in the wiki FAQ) for the same; none found, reasoned why (settings live in a desktop companion app,
  not an on-device legend) in §12.3.
- BMW iDrive bottom-bar convention: https://www.reddit.com/r/AndroidAuto/comments/1x3lmzu/
  bmw_idrive_bottom_bar_controls_disappears_after/ — indirect/corroborating confirmation only
  (medium confidence, flagged as such in §12.3); no primary BMW UX documentation of the bottom bar
  itself was found in this pass.

---

## 13. BT pairing fix: keyboard peripherals should not require a kiosk-side confirm

Research date: 2026-11-16. Scope: diagnose the reported real-device bug ("pairing with a BT
keyboard generates a passkey, fine if the device is a phone, but not if it is a keyboard") against
the already-shipped agent (`f64a01a`) and frontend (`b4ae616`/`867f85e`), and recommend an exact,
implementable fix. **Design only, no code written.**

### 13.1 Code confirmed first, in full, not just the snippet in the task brief

Read `prototype/control-surface/agent/src/bluetooth.rs` directly, in full (the actual `Agent1`
implementation lives here, not inline in `main.rs` — `main.rs` only relays the resulting
`AgentEvent` onto WS). Confirmed exactly what exists:

- `request_confirmation()` (lines 105-124): real WS round-trip, blocks up to `CONFIRM_TIMEOUT`
  (30s) on the reply — this is the one that produces the kiosk confirm screen.
- `display_passkey()` (lines 126-130): notify-only, broadcasts `AgentEvent::DisplayPasskey`, no
  reply, no blocking.
- `request_authorization()`/`authorize_service()` (lines 141-157): auto-accept, already a stated,
  deliberate shortcut (reconnect/trusted-device path, not first-pairing).
- `request_pin_code()` (lines 159-166) and `request_passkey()` (lines 168-175) **are implemented**
  (contrary to the task brief's framing that they might not be) — both **unconditionally reject**
  with `org.bluez.Error.Failed`, logged as "not supported by this agent." `request_passkey()`
  specifically is the callback that would fire on us if the negotiated method required *us* to
  type in a six-digit code (e.g. if our capability were ever `KeyboardOnly`) — not the active path
  for this bug, since our capability is `KeyboardDisplay`, but confirmed implemented-and-rejecting,
  not missing.
- `cancel()` (lines 94-99) is implemented, clears `pending_confirm` so a stale reply can't land on
  a dead request.
- Agent capability (line 39): `const AGENT_CAPABILITY: &str = "KeyboardDisplay";` — confirmed
  exactly as the task brief states.

**No `Device1.Class`/`Icon`/`UUIDs` read happens anywhere in this file today** — `pair()` (lines
339-355) calls `Device1.Pair()` directly with zero device-type inspection beforehand. This is the
gap this section's fix targets.

### 13.2 What the real Bluetooth spec table says should happen — confirmed, primary source

**Primary source, fetched directly: Bluetooth Core Specification, Part H (Security Manager
Specification), §2.3, Table 2.5 "Selecting key generation method"**
(bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host/
security-manager-specification.html, current hosted copy of the spec). This is the LE Security
Manager's own copy of the IO-capability mapping table; **the classic BR/EDR Secure Simple Pairing
table (Vol 3, Part C, §5.2.2.6) is structurally identical in shape** (same five capability names,
same row/column semantics) — confirmed independently by a second primary citation: BlueZ
maintainer Vudentz, in a real GitHub issue thread (bluez/bluez#8, see §13.3), cites "BLUETOOTH CORE
SPECIFICATION Version 5.2 | Vol 3, Part C page 1317" for the classic case while describing the same
kind of capability-combination lookup. Reading the fetched table with **Responder = the row,
Initiator = the column** (BlueZ's own doc and every other source checked agree the device calling
`Device1.Pair()` is the initiator):

- **Row "Keyboard Only" (Responder = the real keyboard) × Column "Keyboard Display" (Initiator =
  our agent, `AGENT_CAPABILITY = "KeyboardDisplay"`) = "Passkey Entry: initiator displays, responder
  inputs — Authenticated."**

This is exactly **our `DisplayPasskey` firing** (we are the initiator, we display), **with the
keyboard's own keystrokes being what confirms it** (the responder "inputs" the digits shown on our
screen) — precisely what the frontend already correctly implements as the toast-only,
no-WS-reply-needed path. **If the real keyboard genuinely advertised `KeyboardOnly`, this bug
could not happen per the spec table** — confirming the task brief's own framing is exactly right:
the spec-correct outcome for a real keyboard is `DisplayPasskey`, not `RequestConfirmation`.

**Confidence: high** — primary current spec text, fetched and quoted directly, cross-confirmed by
a second primary source (a BlueZ maintainer citing the equivalent classic-pairing page number) for
the claim that the two tables (LE SM, classic SSP) are the same shape.

### 13.3 Why `RequestConfirmation` fires anyway for real keyboards — two documented, cited mechanisms, not guessed

Two independent, real, documented mechanisms were found — either (or both) plausibly explains the
user's specific keyboard; this research pass could not distinguish which without a live `btmon`
capture against the actual unit (flagged, not glossed over, §13.6):

**Mechanism A — the keyboard itself doesn't advertise pure `KeyboardOnly` (far more common in
practice than the spec's idealized table implies).** Confirmed from two independent sources:
- blefyi.com's Numeric Comparison reference table (checked directly): Numeric Comparison — which
  is exactly what produces a `RequestConfirmation` call with a yes/no semantic, not a
  type-it-in one — fires whenever **both** sides present `DisplayYesNo`-class capability, including
  the combination **`KeyboardDisplay` (initiator) × `DisplayYesNo` (responder) → Numeric
  Comparison**. A real BLE keyboard that reports `DisplayYesNo` instead of `KeyboardOnly` (a vendor
  firmware/SDK choice, not a spec requirement — common because many BLE HID reference stacks
  default their IO-capability field rather than reasoning about the device's actual physical I/O)
  would trigger exactly this path against our `KeyboardDisplay` agent.
- u-blox's own Bluetooth security app note (fetched directly, u-connectXpress): independently
  documents the same four-model structure (Just Works / Numeric Comparison / Passkey Entry / OOB)
  and that Numeric Comparison requires **both** sides to have "some interactive mechanism (display,
  keyboard, switch, or similar)" — corroborating that a keyboard reporting anything beyond pure
  `KeyboardOnly` (e.g. a status LED or pairing button modeled as a "yes/no" capability by its
  firmware) tips the negotiation into Numeric Comparison against a `KeyboardDisplay` host.

**Mechanism B — BlueZ/the kernel's own real-world IO-capability-to-method translation has a
documented, maintainer-acknowledged mismatch class against the textbook table, specifically for
display/confirmation combinations.** Primary source: **bluez/bluez GitHub issue #8** (fetched
directly, "DisplayOnly IO capability does not work with BR/EDR connections," opened 2020-04-09,
closed 2020-06-23 as "not planned... appears to not be BlueZ's fault," but the technical exchange is
substantive, not dismissive):
- Reporter (jimbroze): a `DisplayOnly` device paired against an Android phone's `KeyboardDisplay`
  got a `RequestConfirmation` call instead of the spec-table-predicted `DisplayPasskey`-only
  behavior, and suspected "the conversion of an incoming device's capability from DisplayKeyboard
  to DisplayYesNo on BR/EDR connections."
- BlueZ maintainer Vudentz's own reply, quoted directly: *"According to the spec the combination of
  IO capabilities shall result in a Passkey Entry authentication method, which afaik is what
  RequestConfirmation is about since it gives you a passkey, the agent then can display to the
  user"* — i.e. **BlueZ's own `Agent1` D-Bus API does not have a perfectly clean 1:1 mapping from
  "spec says Passkey Entry" to "exactly one of `DisplayPasskey`/`RequestConfirmation` fires" in
  every real capability-combination case**; Vudentz further points at the kernel's own translation
  table (`net/bluetooth/smp.c`) as the actual authority for which agent method fires, not a pure
  reading of the spec table by itself.
- **This is a real, maintainer-confirmed class of "wrong callback for the combination" reports
  against actual hardware, not a one-off, and not limited to this exact device/direction** — a
  second corroborating GitHub issue was found (bluez/bluez#2115, "Pairing fails with
  ConnectionAttemptFailed on ASUS ROG Strix Scope II 96 Wireless keyboard (BLE HID)," opened
  2026-05-10, closed stale with no resolution) confirming **real, shipping BLE HID keyboards
  hitting BlueZ pairing problems is an active, recurring category of report**, though that specific
  issue's thread did not contain enough detail to confirm which callback fired for it.

**Confidence: medium-high.** Both mechanisms are grounded in primary/maintainer sources, not
forum speculation, and either is independently sufficient to explain the bug — but **this research
pass did not have access to the user's actual keyboard model or a live `btmon`/`journalctl -u
bluetooth` capture of the real pairing attempt**, so which mechanism (or combination) applies to
this specific unit is not confirmed, only that the bug class itself is real, documented, and not
specific to this project's agent implementation being wrong per se. **Recommend a cheap, concrete
live check before/alongside implementing the fix below:** run `btmon` (or `journalctl -u bluetooth
-f`) during one live pairing attempt with the real keyboard and read the negotiated IO capability
byte BlueZ logs for the remote device — this costs one pairing attempt and would definitively
distinguish Mechanism A from B, but is not required to ship the fix in §13.4, which is agnostic to
which mechanism is actually firing.

### 13.4 Detecting "this is a keyboard/HID peripheral" — the real D-Bus properties, cited

Three independent, real `org.bluez.Device1` signals, all readable **before** calling `Pair()`
(the device object already exists in BlueZ's object tree once it's been seen during a scan, which
is required for it to appear in this project's own device-picker UI per §4 — so these properties
are already available at the exact point `ClientCommand::BtPair` fires):

1. **`Class` (uint32, readonly, optional per BlueZ's own `org.bluez.Device.rst`, fetched directly:
   "The Bluetooth class of device of the remote device").** This is the classic Class-of-Device
   (CoD) 24-bit field. Per the Bluetooth SIG's own assigned-numbers baseband encoding (confirmed
   via two independent, consistent secondary renderings that both cite the SIG spec directly —
   Mozilla's B2G `BluetoothClassOfDevice` wiki page and the `BT-Manager` Python library's `cod.py`,
   both fetched and in agreement): **Major Device Class occupies bits 12-8 (mask `0x1F00`)**, value
   **`0x05` = "Peripheral (mouse, joystick, keyboard, ...)"**; within Peripheral, the **Minor Device
   Class occupies bits 7-2**, and **bits 7-6 specifically (mask `0xC0`) encode `0x40` = Keyboard,
   `0x80` = Pointing device, `0xC0` = Combo keyboard/pointing device**. So: `(class & 0x1F00) ==
   0x0500 && (class & 0xC0) == 0x40` (or `0xC0`, for a combo device) identifies a keyboard by CoD.
   **Caveat, stated plainly: `Class` is marked `optional` in BlueZ's own doc, and BLE-only devices
   (no classic BR/EDR radio) commonly do not populate it at all** — this property alone is not a
   reliable universal signal, only a strong one when present.
2. **`Icon` (string, readonly, optional, per the same `org.bluez.Device.rst`).** BlueZ derives this
   from the CoD/GAP-appearance data using the freedesktop.org icon-naming convention; the
   project-brief-asserted value `input-keyboard` is the correct, standard icon name BlueZ uses for
   keyboard-class devices — **this is general, well-established BlueZ behavior, not independently
   re-verified against BlueZ's exact icon-generation source in this research pass** (flagged medium
   confidence on this one specific sub-claim, unlike the CoD bit layout and UUIDs below, which were
   independently cross-confirmed from multiple primary/semi-primary sources).
3. **`UUIDs` (array of strings, the device's advertised GATT/SDP service UUIDs) containing the HID
   service UUID.** Confirmed via two independent, real open-source BT-keyboard-emulation projects
   read directly (not just referenced): `HID_UUID = "00001124-0000-1000-8000-00805f9b34fb"` (the
   **classic BR/EDR HID Profile** UUID, `0x1124`, used identically in both `btkey`'s `btlink.py` and
   `badbt`'s `btk_server.py`), and a third project (`pi-ble-keyboard`, fetched directly) confirming
   the **separate BLE HOGP (HID-over-GATT) primary service UUID is `0x1812`** ("HID Information
   (0x2A4A)... HID Report Map (0x2A4B)..." all children of the `0x1812` HID Service) — **a real BT
   keyboard may advertise either or both, depending on whether it's classic or LE**, so a robust
   check must look for `0x1124` **or** `0x1812` in `UUIDs`, not just one.

**Recommendation on which signal(s) to use, and why: check all three, OR'd together, treat any one
hit as "this is a keyboard/HID peripheral."** None of the three is independently 100% reliable
(`Class` is optional and BR/EDR-only in practice; `Icon` depends on BlueZ's own derivation being
correct and is the one sub-claim not independently re-verified here; `UUIDs` requires the scan to
have already resolved the device's advertised services, which it should have by the time a user
has selected it from the picker to pair, per §4's discovered-device flow) — but **a match on any one
of them is a strong, low-false-positive signal that a phone or speaker would essentially never
produce** (phones don't advertise CoD Peripheral/Keyboard, the `input-keyboard` icon, or UUID
`0x1124`/`0x1812`), so OR-combining them maximizes coverage for the narrow "detect specifically
keyboards/HID input peripherals" goal without needing all three to agree.

### 13.5 The recommended fix — precise, scoped to HID detection, not a blanket behavior change

**Recommendation: option (a) from the task brief — detect HID/keyboard devices via
`Class`/`Icon`/`UUIDs` immediately before calling `Device1.Pair()`, and auto-accept any
`RequestConfirmation` that arrives during that specific pairing attempt without showing the
frontend confirm screen, while leaving `DisplayPasskey` completely untouched (it already is the
correct, already-working, notify-only case for a keyboard to finish by typing the digits in).**

**Why this, not a blanket "always auto-accept `RequestConfirmation`" change:** `RequestConfirmation`
is also the real, meaningful numeric-comparison yes/no flow for phones and speakers (the task
brief's own framing, and §13.2's spec table confirms `DisplayYesNo`/`KeyboardDisplay`-class remote
devices legitimately use it) — auto-accepting it unconditionally for every device would silently
remove the one real MITM-protection step this project's agent currently provides for those devices,
which is a meaningful security regression the task brief did not ask for and this document does
not recommend. **Scoping the auto-accept to detected-HID-peripheral devices only is the
narrowly-correct fix**: a keyboard structurally cannot satisfy an on-screen yes/no prompt anyway (no
display, no way to signal "yes" except typing — which is exactly what `DisplayPasskey`'s flow
already lets it do), so skipping confirmation for *that specific class of device* loses no real
protection (there was never a way for the keyboard to meaningfully confirm in the first place),
while every other device keeps the exact real confirm screen it has today.

**Exact implementation, file/function pointers:**

1. **`prototype/control-surface/agent/src/bluetooth.rs` — new helper function, placed near
   `device_path_to_address()`/`address_to_device_path()` (around line 185):**
   ```
   fn device_is_hid_keyboard(conn: &Connection, address: &str) -> bool {
       let path = address_to_device_path(address);
       let Ok(proxy) = zbus::blocking::Proxy::new(conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
       else { return false; };

       // Signal 1: classic Class-of-Device, major=Peripheral (0x05), minor bits 7-6 = Keyboard/Combo.
       if let Ok(class) = proxy.get_property::<u32>("Class") {
           if (class & 0x1F00) == 0x0500 && (class & 0xC0) != 0 {
               return true;
           }
       }
       // Signal 2: BlueZ-derived icon name.
       if let Ok(icon) = proxy.get_property::<String>("Icon") {
           if icon == "input-keyboard" {
               return true;
           }
       }
       // Signal 3: advertised HID service UUIDs (classic HID Profile 0x1124, BLE HOGP 0x1812).
       if let Ok(uuids) = proxy.get_property::<Vec<String>>("UUIDs") {
           let hit = uuids.iter().any(|u| {
               let u = u.to_lowercase();
               u.starts_with("00001124-") || u.starts_with("00001812-")
           });
           if hit {
               return true;
           }
       }
       false
   }
   ```
   Every branch degrades to `false` on a missing/unreadable property (optional properties,
   per §13.4) rather than erroring — matches this file's existing "never panic on a D-Bus shrug"
   convention (e.g. `value_to_string`'s `None` fallback).

2. **`BtHandle` struct (line 72-76) and `Agent` struct (line 83-86) — add one shared flag,**
   alongside the existing `pending_confirm: Arc<Mutex<Option<Sender<bool>>>>` pattern:
   `auto_confirm_hid: Arc<Mutex<bool>>`. Threaded into `Agent` the same way `pending_confirm`
   already is, and returned from `connect_and_register_agent()` as part of `BtHandle` the same way.

3. **`pair()` (lines 339-355) — set the flag right before calling `Device1.Pair()`:**
   ```
   pub fn pair(handle: &BtHandle, address: &str, events: &Sender<AgentEvent>) {
       let is_hid = device_is_hid_keyboard(&handle.conn, address);
       *handle.auto_confirm_hid.lock().unwrap() = is_hid;
       if is_hid {
           println!("[bridge] bt: {address} detected as HID/keyboard peripheral (Class/Icon/UUIDs) \
                      -- RequestConfirmation will auto-accept for this pairing attempt");
       }
       let path = address_to_device_path(address);
       // ...existing Pair() call unchanged...
   }
   ```
   The flag is scoped to "this specific in-flight pairing attempt," matching `BtHandle`'s own
   documented one-at-a-time pairing model (line 70-71) — no risk of a stale `true` leaking into a
   later phone-pairing attempt, since `pair()` always re-evaluates and re-sets it at the top of
   every call, for every address, every time.

4. **`Agent::request_confirmation()` (lines 105-124) — check the flag first, before the WS
   round-trip:**
   ```
   fn request_confirmation(&self, device: ObjectPath<'_>, passkey: u32) -> zbus::fdo::Result<()> {
       let address = device_path_to_address(device.as_str());
       if *self.auto_confirm_hid.lock().unwrap() {
           println!("[bridge] bt agent: RequestConfirmation device={address} passkey={passkey} \
                      -- AUTO-ACCEPTING (detected HID/keyboard peripheral, no kiosk confirm \
                      screen shown, see bluetooth.rs's device_is_hid_keyboard())");
           return Ok(());
       }
       // ...existing WS round-trip logic, completely unchanged, for every other device...
   }
   ```
   No `AgentEvent::ConfirmRequest` is sent in the auto-accept branch, so `main.rs`'s
   `relay_bt_events()` never broadcasts `bt_pairing_confirm_request`, and the frontend's confirm
   screen (`app.js` lines ~374-384) never opens for this device — exactly the fix the task brief
   asked for, with zero frontend changes needed (the frontend already does the right thing with
   whichever messages actually arrive; this fix just stops the wrong message from being sent for
   keyboards).

5. **`display_passkey()` (lines 126-130): no change.** It already is the correct, already-working,
   notify-only path for the case where the keyboard legitimately negotiates `KeyboardOnly` and the
   spec-correct method fires — this fix adds a guard rail for the *mis-negotiated*/non-conformant
   case (§13.3's Mechanisms A/B), it does not touch the already-correct case at all.

**What this fix deliberately does not attempt:** it does not try to distinguish *which* of
Mechanism A or B (§13.3) is firing for any given keyboard, and it does not need to — the fix is
agnostic to the root cause, because it intercepts at the one point both mechanisms funnel through
(`RequestConfirmation` actually firing on our agent), for the one class of device
(keyboard/HID) where that callback is structurally un-satisfiable by the device itself.

### 13.6 Open items, stated plainly

1. **Which of §13.3's Mechanism A/B actually fires for the user's specific keyboard model is not
   confirmed** — a live `btmon`/`journalctl -u bluetooth` capture during one real pairing attempt
   would settle it, but is not required to implement or validate the fix in §13.5, which works
   regardless of which mechanism is the cause.
2. **The `Icon == "input-keyboard"` claim (§13.4, signal 2) is the one sub-claim in this section not
   independently re-verified against BlueZ's own icon-derivation source** — low risk (it's a
   widely-cited, standard BlueZ convention), but flagged rather than silently treated as fully
   confirmed, matching this document's own established practice elsewhere (§5.1, §8.1, §8.2).
3. **This fix has not been tested against the real device/keyboard** — this research pass had no
   live hardware access (consistent with every other research-only section of this document); the
   code shapes in §13.5 are precise enough to implement directly but should be validated against
   the actual failing keyboard before being considered closed.

### Sources
- `prototype/control-surface/agent/src/bluetooth.rs` (read directly, in full, this revision) — the
  real current `Agent1` implementation (§13.1), confirming what's implemented/rejecting vs. missing,
  and the exact `pair()`/`request_confirmation()` insertion points for §13.5.
- https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host/
  security-manager-specification.html — Bluetooth Core Spec Part H, Table 2.5, the primary
  IO-capability mapping table, fetched and quoted directly (§13.2).
- https://github.com/bluez/bluez/issues/8 — "DisplayOnly IO capability does not work with BR/EDR
  connections," BlueZ maintainer Vudentz's own reply quoted directly, the primary real-world
  evidence for Mechanism B (§13.3).
- https://github.com/bluez/bluez/issues/2115 — "Pairing fails... ASUS ROG Strix Scope II 96
  Wireless keyboard (BLE HID)," corroborating that real BLE keyboards hit BlueZ pairing issues as
  an active, recurring report category (§13.3).
- https://blefyi.com/glossary/numeric-comparison/ — Numeric Comparison IO-capability-combination
  table, corroborating Mechanism A (§13.3).
- https://content.u-blox.com/sites/default/files/u-connectXpress-BluetoothSecurity_AppNote_UBX-16022676.pdf
  — independent corroboration of the four-pairing-model structure and Numeric Comparison's
  both-sides-interactive requirement (§13.3).
- https://bluez.readthedocs.io/en/latest/device-api, https://github.com/bluez/bluez/blob/
  52bda9d45572ab4629bf2f686616c0398f489dad/doc/org.bluez.Device.rst — `Device1.Class`/`Icon`/`UUIDs`
  property definitions, fetched directly (§13.4).
- http://wiki.mozilla.org/B2G/Bluetooth/WebBluetooth-v2/BluetoothClassOfDevice,
  https://pythonhosted.org/BT-Manager/_modules/bt_manager/cod.html — independent, consistent
  renderings of the Bluetooth SIG assigned-numbers Class-of-Device bit layout (major class 0x05 =
  Peripheral, minor bits 7-6 = Keyboard/Pointing/Combo), both fetched directly (§13.4).
- https://raw.githubusercontent.com/npitre/btkey/main/btkey/btlink.py,
  https://github.com/Shubhamvis98/badbt/blob/master/btk_server.py — independent real projects using
  HID UUID `0x1124` for classic BR/EDR HID, fetched directly (§13.4).
- https://github.com/ebowwa/pi-ble-keyboard — independent real project confirming the separate BLE
  HOGP HID service UUID `0x1812`, fetched directly (§13.4).
