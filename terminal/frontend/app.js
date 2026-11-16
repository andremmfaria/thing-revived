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
const BRIDGE_WS_URL = 'ws://localhost:8765';

// Real Linux KEY_* codes on /dev/input/event0, confirmed live (see
// docs/research/button-knob-clock-design.md §1/§7):
//   KEY_ESC=1, KEY_1..KEY_4=2..5, KEY_ENTER=28, KEY_M=50.
// Buttons 1-4 map directly to a session index (requirement 1) - NOT
// next/prev. KEY_M and KEY_ENTER are intentionally absent: their behavior
// is unchanged and arrives through the normal keydown path (replayed by
// control-surface/agent's uinput passthrough), exactly as before. KEY_ESC
// is also absent: it's a hard-recovery control handled entirely inside
// control-surface/agent (systemctl restart of terminal-agent) with no
// frontend involvement at all.
const BUTTON_SESSION_MAP = {
  2: 0, // KEY_1 -> session 0
  3: 1, // KEY_2 -> session 1
  4: 2, // KEY_3 -> session 2
  5: 3, // KEY_4 -> session 3
};
const KNOB_SCROLL_LINES_PER_STEP = 2;

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
};

// Single shared xterm.js Terminal instance, reused across all session tabs
// (not one per session) -- on a tab switch, renderScreen() resets it and
// replays that session's buffered chunks. disableStdin is true because
// keyboard input is handled entirely by the existing onKeyDown() listener
// below (PTY input goes over the WS 'input' message, not through xterm's
// own onData), so xterm here is purely an output renderer + VT100/curses
// interpreter, not an input widget.
const term = new Terminal({
  cols: 80,
  rows: 24,
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

function connectAgent() {
  const ws = new WebSocket(AGENT_WS_URL);
  state.agentSocket = ws;
  ws.onopen = function () {
    state.agentReady = true;
    setStatus('agent-dot', 'agent-text', true, 'AGENT: LIVE');
    sendAgent({ type: 'resize', session: state.activeSession, cols: 80, rows: 24 });
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
  // Read-only consumer of the existing bridge daemon's protocol
  // ({"type":"button",...} / {"type":"knob",...}), documented in
  // docs/research/hardware-interaction.md. If the bridge isn't running
  // (e.g. during frontend-only dev iteration), this just keeps retrying
  // and the terminal still works from on-screen/keyboard input alone.
  const ws = new WebSocket(BRIDGE_WS_URL);
  ws.onopen = function () {
    setStatus('bridge-dot', 'bridge-text', true, 'BRIDGE: LIVE');
  };
  ws.onclose = function () {
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
      scrollActiveScreen(msg.delta);
    } else if (msg.type === 'button' && msg.value === 1) {
      if (Object.prototype.hasOwnProperty.call(BUTTON_SESSION_MAP, msg.code)) {
        switchToSession(BUTTON_SESSION_MAP[msg.code]);
      }
      // KEY_M (50) / KEY_ENTER (28) / KEY_ESC (1) intentionally unhandled
      // here: M/Enter arrive through the normal keydown path (uinput-
      // replayed by control-surface/agent); ESC's unwedge action happens
      // entirely inside control-surface/agent, no frontend role at all.
    }
  };
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
  sendAgent({ type: 'resize', session: state.activeSession, cols: 80, rows: 24 });
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
  // Capture phase: onKeyDown must see every physical/keyboard keydown
  // before xterm.js's own internal textarea listener (bubble phase) can
  // stopPropagation() on it -- preserves the exact pre-existing
  // keydown-input logic (sendAgent 'input' messages) unchanged, with
  // disableStdin above as the second line of defense against xterm also
  // trying to handle the same keystroke.
  screen.addEventListener('keydown', onKeyDown, { capture: true });
  document.querySelector('.term-shell').addEventListener('click', focusScreen);
  document.querySelector('.term-shell').addEventListener('touchstart', focusScreen, { passive: true });
  focusScreen();
  connectAgent();
  connectBridge();
  setInterval(renderClock, 1000);
}

main();
