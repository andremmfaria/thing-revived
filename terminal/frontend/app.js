/*
 * Terminal frontend. Deliberately plain ES6 (arrow functions, let/const,
 * template literals) and NO ES2020 syntax (optional chaining `?.`,
 * nullish coalescing `??`) for consistency with the sibling dashboard/ and
 * control-surface/ frontends on this device. This file is a non-module
 * classic <script>: a file:// page load has no Content-Type header, so a
 * module script tag fails to load at all -- same reason vendor/xterm.js
 * (loaded just before this file) is the UMD build, not the ESM one.
 *
 * Rendering is real xterm.js (vendor/xterm.js + vendor/xterm.css) writing
 * straight from the WebSocket's 'output' messages via term.write(). The
 * WS JSON protocol itself (documented in docs/terminal.md) is unchanged --
 * terminal/agent's main.rs needed zero changes for this.
 */

// terminal/agent's own WebSocket, PTY I/O + clock_sync.
const AGENT_WS_URL = 'ws://localhost:8766';
// The EXISTING bridge/ daemon's WebSocket (hardware buttons + knob),
// listened to read-only here for session-chrome control. Deliberately
// UNTOUCHED on the daemon side -- see docs/terminal.md for why button
// code -> action mapping is done here, client-side, instead of in bridge/.
// As of docs/research/settings-menu-feasibility.md this connection is also
// used to drive the settings menu (BT/brightness/timezone) -- same socket,
// more message types, per that doc's §9.2 "reuse the existing connection"
// recommendation.
const BRIDGE_WS_URL = 'ws://localhost:8765';

// Real Linux KEY_* codes on /dev/input/event0, confirmed live (see
// docs/research/button-knob-clock-design.md §1/§7):
//   KEY_ESC=1, KEY_1..KEY_4=2..5, KEY_ENTER=28, KEY_M=50.
// Buttons 1-4 map directly to a session index (requirement 1) - NOT
// next/prev. KEY_M and KEY_ENTER are intentionally absent: their behavior
// is unchanged and arrives through the normal keydown path (replayed by
// control-surface/agent's uinput passthrough), exactly as before.
// KEY_ESC (code 1) now toggles the settings menu (settings-menu-feasibility
// .md §2.2/§3) -- the hard-recovery job it used to do moved to KEY_M.
const BUTTON_SESSION_MAP = {
  2: 0, // KEY_1 -> session 0
  3: 1, // KEY_2 -> session 1
  4: 2, // KEY_3 -> session 2
  5: 3, // KEY_4 -> session 3
};
const KEY_ESC = 1;
const KEY_1 = 2;
const KEY_ENTER = 28;
const KNOB_SCROLL_LINES_PER_STEP = 2;

// Fixed, on-device font choices for the menu's "Terminal > Font family"
// entry. Deliberately just the individual names already present in this
// project's existing CSS stack (style.css's `font-family` on body, and
// this file's own Terminal() constructor below) -- nothing new is
// vendored (terminal/frontend/vendor/ has no font files, only xterm.js
// itself), so these are exactly the names already being relied on via
// browser/OS `local()` resolution, not invented choices.
const FONT_FAMILIES = ['JetBrains Mono', 'Consolas', 'Courier New', 'monospace'];

const state = {
  sessions: [],       // [{index, name}]
  buffers: {},         // index -> array of output chunks (strings), full history
                        // per session so switching tabs can replay it into
                        // the single shared xterm.js instance below.
  activeSession: 0,
  agentSocket: null,
  agentReady: false,
  deadSessions: {},    // index -> true once 'exited' arrives with needs_manual_respawn
  clockOffsetMs: 0,
  clockStatus: 'unsynced',

  // --- Settings menu (docs/research/settings-menu-feasibility.md) ---
  bridgeSocket: null,
  bridgeReady: false,
  menuOpen: false,
  menu: {
    stack: ['root'],      // navigation stack of screen ids, root always at [0]
    highlight: {},         // screenId -> highlighted row index
    adjusting: false,      // true while a range entry (brightness/font size) is live-editing
    escPressedAt: null,    // Date.now() at KEY_ESC press, while menu open (§12.1 tap/hold timing)
    bt: { powered: null, devices: [], pairing: null }, // pairing: {address, passkey, kind: 'confirm'|'display'}
    brightness: { value: null, max: null },
    timezone: { current: null, list: [], loaded: false, group: null },
    fontSize: { value: 13, min: 8, max: 28, step: 1 },
    fontFamilyIndex: 0,
    toast: null,           // {text, until}
  },
};

// Single shared xterm.js Terminal instance, reused across all session tabs
// (not one per session) -- on a tab switch, renderScreen() resets it and
// replays that session's buffered chunks. disableStdin is true because
// keyboard input is handled entirely by the existing onKeyDown() listener
// below (PTY input goes over the WS 'input' message, not through xterm's
// own onData), so xterm here is purely an output renderer + VT100/curses
// interpreter, not an input widget.
const term = new Terminal({
  cursorBlink: true,
  disableStdin: true,
  convertEol: true,
  fontFamily: "'JetBrains Mono', Consolas, 'Courier New', monospace",
  fontSize: 13,
  lineHeight: 1.3,
  theme: {
    background: '#05070a',
    foreground: '#c9dcea',
    cursor: '#35e38a',
  },
});
state.menu.fontSize.value = term.options.fontSize;

// Computes real cols/rows from #screen's actual pixel size + the
// terminal's own font metrics, replacing the old hardcoded cols:80,
// rows:24 that was sent on every WS connect/resize/tab-switch -- that
// hardcoding was why glances never filled the kiosk's real screen width
// (800x480 landscape, see docs/terminal.md / weston.ini's
// `mode=480x800@60 transform=rotate-270`) and, via main.rs's
// Session::resize() only firing SIGWINCH when the new size differs from
// the previous one, was also why a freshly-switched-to bash tab never
// redrew its prompt (the agent kept re-sending the same stale 80x24).
const fitAddon = new FitAddon.FitAddon();
term.loadAddon(fitAddon);

// Re-measures the container and, if agent-ready, tells the active
// session's PTY the real size. Safe to call before the WS is open --
// sendAgent() itself is a no-op until agentReady, and the next call
// (on tab switch or the agent's own onopen) will pick up the fitted size.
function fitAndResize() {
  fitAddon.fit();
  sendAgent({ type: 'resize', session: state.activeSession, cols: term.cols, rows: term.rows });
}

function setStatus(dotId, textId, ok, label) {
  const dot = document.getElementById(dotId);
  const text = document.getElementById(textId);
  if (ok) dot.classList.add('ok');
  else dot.classList.remove('ok');
  text.textContent = label;
}

function renderTabs() {
  const bar = document.getElementById('tab-bar');
  bar.innerHTML = '';
  state.sessions.forEach(function (s) {
    const tab = document.createElement('div');
    tab.className = 'tab' + (s.index === state.activeSession ? ' active' : '');
    tab.textContent = s.name;
    bar.appendChild(tab);
  });
}

function appendOutput(sessionIndex, data) {
  if (!state.buffers[sessionIndex]) state.buffers[sessionIndex] = [];
  state.buffers[sessionIndex].push(data);
  // Cap retained text so a long-running session (e.g. glances, which
  // redraws continuously) doesn't grow this page's memory unbounded.
  const MAX_CHUNKS = 2000;
  if (state.buffers[sessionIndex].length > MAX_CHUNKS) {
    state.buffers[sessionIndex].splice(0, state.buffers[sessionIndex].length - MAX_CHUNKS);
  }
  if (sessionIndex === state.activeSession) {
    // Only the active session's output is written live; xterm.js itself
    // interprets the ANSI/VT100 cursor-positioning, color and UTF-8
    // box-drawing escape codes here (curses programs like glances need
    // exactly this, not a raw text dump into a <pre>).
    term.write(data);
  }
}

function renderScreen() {
  // Switching tabs: xterm.js is one shared instance across all sessions,
  // so reset it and replay that session's full buffered history. This is
  // the same "full buffer -> rendered surface" shape the old <pre> dump
  // used, just through xterm.write() instead of raw textContent.
  term.reset();
  const chunks = state.buffers[state.activeSession] || [];
  for (let i = 0; i < chunks.length; i++) {
    term.write(chunks[i]);
  }
  document.getElementById('session-text').textContent =
    'session: ' + (sessionName(state.activeSession) || state.activeSession);
}

function sessionName(index) {
  const match = state.sessions.filter(function (s) { return s.index === index; })[0];
  return match ? match.name : null;
}

function sendAgent(message) {
  if (state.agentSocket && state.agentReady) {
    state.agentSocket.send(JSON.stringify(message));
  }
}

function sendBridge(message) {
  if (state.bridgeSocket && state.bridgeReady) {
    state.bridgeSocket.send(JSON.stringify(message));
  }
}

function connectAgent() {
  const ws = new WebSocket(AGENT_WS_URL);
  state.agentSocket = ws;
  ws.onopen = function () {
    state.agentReady = true;
    setStatus('agent-dot', 'agent-text', true, 'AGENT: LIVE');
    fitAndResize();
  };
  ws.onclose = function () {
    state.agentReady = false;
    setStatus('agent-dot', 'agent-text', false, 'AGENT: DISCONNECTED');
    setTimeout(connectAgent, 2000); // same reconnect-on-drop pattern as ui/'s bridge client
  };
  ws.onerror = function () {
    setStatus('agent-dot', 'agent-text', false, 'AGENT: ERROR');
  };
  ws.onmessage = function (evt) {
    let msg;
    try {
      msg = JSON.parse(evt.data);
    } catch (err) {
      return;
    }
    if (msg.type === 'session_list') {
      state.sessions = msg.sessions || [];
      renderTabs();
      renderScreen();
    } else if (msg.type === 'output') {
      appendOutput(msg.session, msg.data);
    } else if (msg.type === 'exited') {
      if (msg.needs_manual_respawn) {
        state.deadSessions[msg.session] = true;
        appendOutput(msg.session, '\n[session exited -- select this tab again to restart it]\n');
      } else if (msg.auto_respawning) {
        appendOutput(msg.session, '\n[session exited, code=' + msg.code + ' -- restarting...]\n');
      } else {
        appendOutput(msg.session, '\n[session exited, code=' + msg.code + ']\n');
      }
    } else if (msg.type === 'clock_sync') {
      state.clockOffsetMs = msg.offset_ms;
      state.clockStatus = msg.status;
      renderClock();
    }
  };
}

function connectBridge() {
  // Consumer of the existing bridge daemon's protocol
  // ({"type":"button",...} / {"type":"knob",...}), documented in
  // docs/research/hardware-interaction.md, now extended per
  // docs/research/settings-menu-feasibility.md with BT/brightness/
  // timezone message types (confirmed shipped, commit f64a01a). If the
  // bridge isn't running (e.g. during frontend-only dev iteration), this
  // just keeps retrying and the terminal still works from on-screen/
  // keyboard input alone.
  const ws = new WebSocket(BRIDGE_WS_URL);
  state.bridgeSocket = ws;
  ws.onopen = function () {
    state.bridgeReady = true;
    setStatus('bridge-dot', 'bridge-text', true, 'BRIDGE: LIVE');
    // Pull current settings state up front so the menu has real values to
    // show the first time it's opened, instead of an empty "..." flash.
    sendBridge({ type: 'bt_power_get' });
    sendBridge({ type: 'brightness_get' });
    sendBridge({ type: 'timezone_get' });
  };
  ws.onclose = function () {
    state.bridgeReady = false;
    setStatus('bridge-dot', 'bridge-text', false, 'BRIDGE: DISCONNECTED');
    setTimeout(connectBridge, 2000);
  };
  ws.onerror = function () {
    setStatus('bridge-dot', 'bridge-text', false, 'BRIDGE: ERROR');
  };
  ws.onmessage = function (evt) {
    let msg;
    try {
      msg = JSON.parse(evt.data);
    } catch (err) {
      return;
    }
    if (msg.type === 'knob') {
      if (state.menuOpen) {
        menuKnob(msg.delta);
      } else {
        scrollActiveScreen(msg.delta);
      }
    } else if (msg.type === 'button') {
      handleButton(msg.code, msg.value);
    } else {
      handleSettingsEvent(msg);
    }
  };
}

// Tap vs. hold threshold for KEY_ESC while the menu is open
// (settings-menu-feasibility.md §12.1): deliberately much shorter than
// control-surface/agent's own 1500ms KEY_M hard-recovery threshold, since
// this is a routine per-row confirm/back action hit dozens of times per
// session, not a rare destructive recovery gesture. 450ms sits inside the
// 400-500ms band AOSP's own DEFAULT_LONG_PRESS_TIMEOUT has occupied across
// its history (§12.1's cited primary source).
const ESC_HOLD_THRESHOLD_MS = 450;

function handleButton(code, value) {
  if (code === KEY_ESC && state.menuOpen) {
    // Dual-role while the menu is open: tap confirms (same action as a
    // knob-push), hold returns one level (or closes at the top level).
    // evdev values: 1 = press, 0 = release, 2 = autorepeat (ignored mid-hold,
    // decision is made entirely at release).
    if (value === 1) {
      state.menu.escPressedAt = Date.now();
    } else if (value === 0 && state.menu.escPressedAt !== null) {
      const heldMs = Date.now() - state.menu.escPressedAt;
      state.menu.escPressedAt = null;
      if (heldMs < ESC_HOLD_THRESHOLD_MS) {
        menuSelect();
      } else if (state.menu.adjusting || state.menu.stack.length > 1) {
        menuBack();
      } else {
        closeMenu();
      }
    }
    return;
  }
  if (value !== 1) return; // press-only, matching the existing convention
  if (code === KEY_ESC) {
    // Menu closed: unchanged single-action open on press.
    toggleMenu();
    return;
  }
  if (state.menuOpen) {
    if (code === KEY_ENTER) {
      menuSelect();
    } else if (code === KEY_1) {
      menuBack();
    }
    // KEY_2..KEY_4 intentionally ignored while the menu is open
    // (settings-menu-feasibility.md §2.3: "kept simple deliberately").
    return;
  }
  if (Object.prototype.hasOwnProperty.call(BUTTON_SESSION_MAP, code)) {
    switchToSession(BUTTON_SESSION_MAP[code]);
  }
  // KEY_M (50) / KEY_ENTER (28) intentionally unhandled here: they arrive
  // through the normal keydown path (uinput-replayed by control-surface/
  // agent); KEY_M's hard-recovery action happens entirely inside
  // control-surface/agent, no frontend role at all.
}

// Routes every non-button/knob bridge message (settings-menu-feasibility.md's
// new BridgeEvent variants) into menu state. Kept separate from the plain
// 'button'/'knob' cases above since these can arrive whether or not the
// menu is currently open (e.g. a pairing confirm request while the user is
// looking at a terminal session) and most of them should surface a toast
// either way.
function handleSettingsEvent(msg) {
  switch (msg.type) {
    case 'bt_power':
      state.menu.bt.powered = msg.powered;
      break;
    case 'bt_error':
      showToast('BT error (' + msg.context + '): ' + msg.message);
      break;
    case 'bt_scan_started':
      state.menu.bt.devices = [];
      break;
    case 'bt_scan_stopped':
      break;
    case 'bt_device_found': {
      const exists = state.menu.bt.devices.some(function (d) { return d.address === msg.address; });
      if (!exists) {
        state.menu.bt.devices.push({ address: msg.address, name: msg.name });
      }
      break;
    }
    case 'bt_pairing_confirm_request':
      state.menu.bt.pairing = { address: msg.address, passkey: msg.passkey, kind: 'confirm' };
      if (!state.menuOpen) openMenu();
      pushScreen('bt-confirm');
      break;
    case 'bt_pairing_passkey_display':
      showToast('Passkey for ' + msg.address + ': ' + msg.passkey);
      break;
    case 'bt_pair_result':
      showToast(msg.success ? ('Paired with ' + msg.address) : ('Pairing failed: ' + (msg.error || 'unknown error')));
      if (state.menu.stack[state.menu.stack.length - 1] === 'bt-confirm') {
        popScreen();
      }
      break;
    case 'brightness':
      state.menu.brightness.value = msg.value;
      state.menu.brightness.max = msg.max;
      break;
    case 'timezone':
      state.menu.timezone.current = msg.value;
      break;
    case 'timezone_list':
      state.menu.timezone.list = msg.zones || [];
      state.menu.timezone.loaded = true;
      break;
    case 'settings_error':
      showToast('Settings error (' + msg.context + '): ' + msg.message);
      break;
    default:
      return; // unknown/irrelevant message, no re-render needed
  }
  renderMenu();
}

function scrollActiveScreen(delta) {
  // xterm.js owns scrollback internally now; scrollLines() takes a line
  // count directly (positive = down, negative = up), so no more pixel/
  // lineHeight approximation is needed here -- this never touches the
  // agent or the bridge daemon, purely a local xterm scroll.
  term.scrollLines(delta * KNOB_SCROLL_LINES_PER_STEP);
}

// Direct session switch (requirement 1). Also doubles as the manual
// dead-session recovery trigger (design doc §8): pressing the button for
// a tab already known dead sends a respawn control message in addition to
// switching focus to it -- no new hardware trigger or WS message type
// needed, matches "press the tab for the thing that's broken" intuition.
function switchToSession(index) {
  const exists = state.sessions.some(function (s) { return s.index === index; });
  if (!exists) return; // no such session configured; no-op, not a crash
  if (state.deadSessions[index]) {
    sendAgent({ type: 'control', session: index, action: 'respawn' });
    delete state.deadSessions[index];
  }
  if (index === state.activeSession) return;
  state.activeSession = index;
  renderTabs();
  renderScreen();
  // Re-fit (container size is unchanged across tabs here, but this also
  // forces a fresh resize message to the newly active session even when
  // the computed size is identical to what another session already had --
  // see fitAndResize()'s comment on why a resize must actually reach the
  // agent on every switch, not just be skipped as a no-op).
  fitAndResize();
}

// --- Clock widget (design doc §4) ------------------------------------------

function renderClock() {
  const now = new Date(Date.now() + state.clockOffsetMs);
  const pad = function (n) { return (n < 10 ? '0' : '') + n; };
  const text = pad(now.getHours()) + ':' + pad(now.getMinutes()) + ':' + pad(now.getSeconds());
  const ok = state.clockStatus === 'ok';
  setStatus('clock-dot', 'clock-text', ok, text);
  const dot = document.getElementById('clock-dot');
  if (state.clockStatus === 'stale' || state.clockStatus === 'unsynced') {
    dot.classList.add('stale');
  } else {
    dot.classList.remove('stale');
  }
}

// --- Settings menu (docs/research/settings-menu-feasibility.md §2) --------
//
// Navigation model implemented exactly per §2.3 ("settled, specified"):
//   - KEY_ESC (code 1) toggles the whole overlay open/closed.
//   - Knob rotation moves the highlighted row, or (while a range entry is
//     "armed" via a first push) adjusts that entry's live value.
//   - Knob push (KEY_ENTER) selects/enters a row; a second push on an
//     armed range entry disarms it and returns to the row list.
//   - KEY_1 is back/cancel while the menu is open (§2.3); KEY_2..KEY_4 are
//     ignored while the menu is open, deliberately not repurposed (§2.3's
//     "kept simple deliberately", §11 item 8 flags richer shortcuts as a
//     future enhancement, not built here).
// Visual design per §2.4: flat single-column list, reverse-video fill on
// the highlighted row (reusing this project's existing status-dot-style
// bold/filled visual language), two levels deep at most.

// Declarative screen tree. Built as a function (not a static object) since
// several screens need to read live state (current BT/brightness/timezone
// values) at render time rather than once at startup.
function menuScreens() {
  const m = state.menu;
  return {
    root: {
      title: 'Settings',
      entries: [
        { id: 'bt', label: 'Bluetooth', kind: 'nav', target: 'bluetooth' },
        {
          id: 'brightness', label: 'Brightness', kind: 'range',
          value: function () { return m.brightness.value; },
          display: function () {
            if (m.brightness.value === null || !m.brightness.max) return '...';
            return Math.round((m.brightness.value / m.brightness.max) * 100) + '%';
          },
          min: 0,
          max: function () { return m.brightness.max || 160; },
          step: function () { return Math.max(1, Math.round((m.brightness.max || 160) / 20)); },
          onChange: function (v) {
            m.brightness.value = v;
            sendBridge({ type: 'brightness_set', value: v });
          },
        },
        { id: 'terminal', label: 'Terminal', kind: 'nav', target: 'terminal' },
        { id: 'timezone', label: 'Timezone: ' + (m.timezone.current || '...'), kind: 'nav', target: 'timezone-groups' },
      ],
    },
    bluetooth: {
      title: 'Bluetooth',
      entries: [
        {
          id: 'bt-power', label: 'Power', kind: 'toggle',
          display: function () { return m.bt.powered === null ? '...' : (m.bt.powered ? 'ON' : 'OFF'); },
          onToggle: function () { sendBridge({ type: 'bt_set_power', powered: !m.bt.powered }); },
        },
        {
          id: 'bt-pair', label: 'Pair a device...', kind: 'nav', target: 'bt-pair-list',
          onEnter: function () { m.bt.devices = []; sendBridge({ type: 'bt_scan_start' }); },
          onLeave: function () { sendBridge({ type: 'bt_scan_stop' }); },
        },
      ],
    },
    'bt-pair-list': {
      title: 'Pair a device',
      dynamicEntries: function () {
        if (m.bt.devices.length === 0) {
          return [{ id: 'bt-pair-scanning', label: 'Scanning...', kind: 'label' }];
        }
        return m.bt.devices.map(function (d) {
          return {
            id: 'bt-dev-' + d.address, label: d.name || d.address, kind: 'action',
            afterSelect: 'none', // wait for bt_pairing_confirm_request / bt_pair_result
            onSelect: function () { sendBridge({ type: 'bt_pair', address: d.address }); },
          };
        });
      },
    },
    'bt-confirm': {
      title: function () {
        return m.bt.pairing ? ('Pair with ' + m.bt.pairing.address + '? (' + m.bt.pairing.passkey + ')') : 'Pairing';
      },
      dynamicEntries: function () {
        return [
          {
            id: 'bt-confirm-yes', label: 'Accept', kind: 'action', afterSelect: 'pop',
            onSelect: function () { sendBridge({ type: 'bt_pairing_confirm', accept: true }); },
          },
          {
            id: 'bt-confirm-no', label: 'Deny', kind: 'action', afterSelect: 'pop',
            onSelect: function () { sendBridge({ type: 'bt_pairing_confirm', accept: false }); },
          },
        ];
      },
    },
    terminal: {
      title: 'Terminal',
      entries: [
        {
          id: 'font-size', label: 'Font size', kind: 'range',
          value: function () { return m.fontSize.value; },
          display: function () { return String(m.fontSize.value); },
          min: m.fontSize.min, max: m.fontSize.max, step: function () { return m.fontSize.step; },
          onChange: function (v) {
            m.fontSize.value = v;
            term.options.fontSize = v; // live-settable per settings-menu-feasibility.md §6
            fitAndResize();
          },
        },
        { id: 'font-family', label: 'Font family: ' + FONT_FAMILIES[m.fontFamilyIndex], kind: 'nav', target: 'font-family-list' },
      ],
    },
    'font-family-list': {
      title: 'Font family',
      dynamicEntries: function () {
        return FONT_FAMILIES.map(function (f, i) {
          return {
            id: 'font-' + i, label: f, kind: 'action', afterSelect: 'pop',
            onSelect: function () {
              m.fontFamilyIndex = i;
              term.options.fontFamily = "'" + f + "', monospace";
              fitAndResize();
            },
          };
        });
      },
    },
    'timezone-groups': {
      title: 'Timezone region',
      onEnter: function () {
        if (!m.timezone.loaded) sendBridge({ type: 'timezone_list_get' });
      },
      dynamicEntries: function () {
        const groups = timezoneGroups();
        if (groups.length === 0) {
          return [{ id: 'tz-loading', label: 'Loading...', kind: 'label' }];
        }
        return groups.map(function (g) {
          const hasChildren = m.timezone.list.some(function (z) { return z.indexOf(g + '/') === 0; });
          if (hasChildren) {
            return {
              id: 'tz-group-' + g, label: g, kind: 'nav-dynamic', target: 'timezone-zones',
              onEnter: function () { m.timezone.group = g; },
            };
          }
          return {
            id: 'tz-zone-' + g, label: g, kind: 'action', afterSelect: 'root',
            onSelect: function () { sendBridge({ type: 'timezone_set', value: g }); },
          };
        });
      },
    },
    'timezone-zones': {
      title: function () { return m.timezone.group || 'Timezone'; },
      dynamicEntries: function () {
        return zonesInGroup(m.timezone.group).map(function (z) {
          return {
            id: 'tz-' + z, label: z, kind: 'action', afterSelect: 'root',
            onSelect: function () { sendBridge({ type: 'timezone_set', value: z }); },
          };
        });
      },
    },
  };
}

// Groups the 598-entry real timezone list (confirmed live) into IANA
// top-level regions ("Europe", "America", ...) plus bare zone names that
// have no region ("UTC") so the top-level screen is always a short, fixed
// list -- this is the "reasonable filter/scroll" the task leaves to this
// implementation's judgment (settings-menu-feasibility.md §7.1's
// list-timezones Combo shape, adapted for a small kiosk screen rather than
// rendering all 598 names flat).
function timezoneGroups() {
  const list = state.menu.timezone.list;
  const seen = {};
  const groups = [];
  list.forEach(function (z) {
    const slash = z.indexOf('/');
    const g = slash === -1 ? z : z.slice(0, slash);
    if (!seen[g]) {
      seen[g] = true;
      groups.push(g);
    }
  });
  groups.sort();
  return groups;
}

function zonesInGroup(group) {
  const list = state.menu.timezone.list;
  return list.filter(function (z) { return z.indexOf(group + '/') === 0; }).sort();
}

function currentScreenId() {
  const stack = state.menu.stack;
  return stack[stack.length - 1];
}

function currentScreen() {
  return menuScreens()[currentScreenId()];
}

function currentEntries() {
  const screen = currentScreen();
  if (!screen) return [];
  return screen.dynamicEntries ? screen.dynamicEntries() : (screen.entries || []);
}

function currentHighlight() {
  const id = currentScreenId();
  if (typeof state.menu.highlight[id] !== 'number') state.menu.highlight[id] = 0;
  const entries = currentEntries();
  if (state.menu.highlight[id] >= entries.length) state.menu.highlight[id] = Math.max(0, entries.length - 1);
  return state.menu.highlight[id];
}

function pushScreen(id, onEnterOverride) {
  state.menu.stack.push(id);
  state.menu.adjusting = false;
  const screen = menuScreens()[id];
  if (onEnterOverride) onEnterOverride();
  if (screen && screen.onEnter) screen.onEnter();
  renderMenu();
}

function popScreen() {
  if (state.menu.stack.length <= 1) return;
  const leavingId = state.menu.stack.pop();
  const leavingScreen = menuScreens()[leavingId];
  if (leavingScreen && leavingScreen.onLeave) leavingScreen.onLeave();
  state.menu.adjusting = false;
  renderMenu();
}

function popToRoot() {
  while (state.menu.stack.length > 1) popScreen();
}

function openMenu() {
  state.menuOpen = true;
  document.getElementById('menu-overlay').classList.remove('hidden');
  renderMenu();
}

function closeMenu() {
  // Unwind any entered sub-screens so the next open starts clean, running
  // each screen's onLeave (e.g. stops an in-progress BT scan).
  popToRoot();
  state.menuOpen = false;
  state.menu.adjusting = false;
  document.getElementById('menu-overlay').classList.add('hidden');
}

function toggleMenu() {
  if (state.menuOpen) closeMenu();
  else openMenu();
}

function menuKnob(delta) {
  const entries = currentEntries();
  if (entries.length === 0) return;
  const idx = currentHighlight();
  const entry = entries[idx];
  if (state.menu.adjusting && entry && entry.kind === 'range') {
    const step = typeof entry.step === 'function' ? entry.step() : entry.step;
    const min = typeof entry.min === 'function' ? entry.min() : entry.min;
    const max = typeof entry.max === 'function' ? entry.max() : entry.max;
    let v = entry.value() + delta * step;
    if (v < min) v = min;
    if (v > max) v = max;
    entry.onChange(v);
    renderMenu();
    return;
  }
  // Move the highlight one row per knob message, ignoring magnitude for
  // list navigation (large fast-spin deltas would otherwise skip rows
  // unpredictably) -- fine-grained magnitude is only meaningful for range
  // adjustment above.
  const dir = delta > 0 ? 1 : (delta < 0 ? -1 : 0);
  let next = idx + dir;
  if (next < 0) next = 0;
  if (next > entries.length - 1) next = entries.length - 1;
  state.menu.highlight[currentScreenId()] = next;
  renderMenu();
}

function menuSelect() {
  const entries = currentEntries();
  const idx = currentHighlight();
  const entry = entries[idx];
  if (!entry) return;
  if (entry.kind === 'label') return;
  if (entry.kind === 'toggle') {
    entry.onToggle();
    return;
  }
  if (entry.kind === 'range') {
    // First push arms live-adjust; second push disarms (value is already
    // applied live on every knob step, per §6's "no restart" confirmation,
    // so there is nothing further to "commit" here).
    state.menu.adjusting = !state.menu.adjusting;
    renderMenu();
    return;
  }
  if (entry.kind === 'nav' || entry.kind === 'nav-dynamic') {
    pushScreen(entry.target, entry.onEnter);
    return;
  }
  if (entry.kind === 'action') {
    entry.onSelect();
    if (entry.afterSelect === 'root') popToRoot();
    else if (entry.afterSelect === 'pop' || !entry.afterSelect) popScreen();
    // afterSelect === 'none': caller is responsible for navigating away
    // once an async result arrives (e.g. bt_pair_result).
    return;
  }
}

function menuBack() {
  if (state.menu.adjusting) {
    state.menu.adjusting = false;
    renderMenu();
    return;
  }
  popScreen();
}

function showToast(text) {
  state.menu.toast = text;
  renderMenu();
  clearTimeout(state.menu._toastTimer);
  state.menu._toastTimer = setTimeout(function () {
    state.menu.toast = null;
    renderMenu();
  }, 4000);
}

function renderMenu() {
  const titleEl = document.getElementById('menu-title');
  const listEl = document.getElementById('menu-list');
  const toastEl = document.getElementById('menu-toast');

  if (toastEl) {
    if (state.menu.toast) {
      toastEl.textContent = state.menu.toast;
      toastEl.classList.remove('hidden');
    } else {
      toastEl.classList.add('hidden');
    }
  }

  if (!state.menuOpen) return;

  const screen = currentScreen();
  if (!screen || !titleEl || !listEl) return;
  titleEl.textContent = typeof screen.title === 'function' ? screen.title() : screen.title;

  const entries = currentEntries();
  const highlight = currentHighlight();
  listEl.innerHTML = '';
  entries.forEach(function (entry, i) {
    const row = document.createElement('div');
    row.className = 'menu-row' + (i === highlight ? ' sel' : '') + (i === highlight && state.menu.adjusting ? ' adjusting' : '');
    const label = document.createElement('span');
    label.className = 'menu-row-label';
    label.textContent = entry.label;
    row.appendChild(label);
    if (entry.kind === 'toggle' || entry.kind === 'range') {
      const value = document.createElement('span');
      value.className = 'menu-row-value';
      value.textContent = entry.display();
      row.appendChild(value);
    }
    // Touch is a secondary input per §2.3: tapping a row selects it the
    // same way a knob-push would, same DOM element, no separate layout.
    row.addEventListener('click', function () {
      state.menu.highlight[currentScreenId()] = i;
      menuSelect();
    });
    listEl.appendChild(row);
  });

  const hintEl = document.getElementById('menu-hint');
  if (hintEl) {
    hintEl.textContent = menuHintText();
  }
}

// Contextual hint-bar text (settings-menu-feasibility.md §12.3), extended
// from the previous 2-branch stack-depth check to 4 branches reflecting
// §12.1's new ESC tap=confirm/hold=context-dependent-return semantics.
// Checked in order: adjusting a range value, the BT pairing confirm/deny
// prompt, any non-root list/nav screen, else the top-level list.
function menuHintText() {
  if (state.menu.adjusting) {
    return 'knob: adjust \u00b7 esc: confirm \u00b7 esc (hold): cancel';
  }
  if (currentScreenId() === 'bt-confirm') {
    return 'knob: move \u00b7 esc: confirm selection \u00b7 esc (hold): cancel pairing';
  }
  if (state.menu.stack.length > 1) {
    return 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): back';
  }
  return 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): close';
}

// --- Keyboard input -------------------------------------------------------
// Per docs/research/bt-hid-terminal-feasibility.md's design conclusion: a
// paired BT keyboard surfaces as a normal evdev node that Chromium's own
// native input stack turns into ordinary keydown/keyup DOM events,
// exactly like the touchscreen already bypasses the bridge daemon. So
// this is a completely standard browser keydown handler, no new evdev
// code needed here at all. Buttons 1-4 no longer reach this path at all
// (control-surface/agent grabs event0 exclusively), so there is nothing
// here to filter for them.

const KEY_SEQUENCES = {
  Enter: '\r',
  Backspace: '\x7f',
  Tab: '\t',
  Escape: '\x1b',
  ArrowUp: '\x1b[A',
  ArrowDown: '\x1b[B',
  ArrowRight: '\x1b[C',
  ArrowLeft: '\x1b[D',
};

function onKeyDown(evt) {
  // Input isolation while the menu is open (settings-menu-feasibility.md
  // §12.2): buttons 1-4 and the knob are already gated by state.menuOpen in
  // handleButton()/connectBridge()'s onmessage, but a uinput-replayed
  // KEY_ENTER keystroke (or a real BT-keyboard keystroke) still arrives
  // here as a plain DOM keydown and would otherwise type into the PTY even
  // while the menu overlay is showing. Gate at the top, upstream of every
  // existing branch below, so no individual key-mapping needs its own
  // menu-aware special case.
  if (state.menuOpen) {
    evt.preventDefault();
    return;
  }
  let data = null;
  if (evt.ctrlKey && evt.key.length === 1) {
    // Ctrl+A..Ctrl+Z -> control bytes 0x01..0x1a, the standard terminal mapping.
    const code = evt.key.toUpperCase().charCodeAt(0);
    if (code >= 65 && code <= 90) {
      data = String.fromCharCode(code - 64);
    }
  } else if (KEY_SEQUENCES[evt.key]) {
    data = KEY_SEQUENCES[evt.key];
  } else if (evt.key.length === 1) {
    data = evt.key;
  }
  if (data !== null) {
    evt.preventDefault();
    sendAgent({ type: 'input', session: state.activeSession, data: data });
  }
}

// Touchscreen tap/click focus fix: the touchscreen itself works (confirmed
// live), but tapping it doesn't focus the element that captures keystrokes
// (#screen, now xterm.js's container). Without focus, a physical BT
// keyboard's keydown events (or anything relying on #screen having focus)
// have nowhere to land after a touch interaction steals focus elsewhere
// (or leaves nothing focused at all). Focus #screen on every tap/click
// inside the terminal container, not just at startup. term.focus() is
// xterm's own helper (focuses its internal hidden textarea); the plain
// #screen div itself has tabindex=0 as a fallback keydown target even
// before xterm attaches, in case onKeyDown below is reached that way.
function focusScreen() {
  term.focus();
  document.getElementById('screen').focus();
}

function main() {
  const screen = document.getElementById('screen');
  term.open(screen);
  fitAddon.fit();
  // Kiosk window is fixed-size in practice, but re-fit on any resize
  // (orientation/DPI change, devtools-attached debugging, etc.) rather
  // than assuming the viewport never changes after load.
  window.addEventListener('resize', fitAndResize);
  // Capture phase: onKeyDown must see every physical/keyboard keydown
  // before xterm.js's own internal textarea listener (bubble phase) can
  // stopPropagation() on it -- preserves the exact pre-existing
  // keydown-input logic (sendAgent 'input' messages) unchanged, with
  // disableStdin above as the second line of defense against xterm also
  // trying to handle the same keystroke.
  screen.addEventListener('keydown', onKeyDown, { capture: true });
  document.querySelector('.term-shell').addEventListener('click', focusScreen);
  document.querySelector('.term-shell').addEventListener('touchstart', focusScreen, { passive: true });
  const closeBtn = document.getElementById('menu-close');
  if (closeBtn) closeBtn.addEventListener('click', closeMenu);
  focusScreen();
  connectAgent();
  connectBridge();
  setInterval(renderClock, 1000);
}

main();
