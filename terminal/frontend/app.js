/*
 * Terminal frontend placeholder. Deliberately plain ES6 (arrow functions,
 * let/const, template literals) and NO ES2020 syntax (optional chaining
 * `?.`, nullish coalescing `??`) -- Chromium 69, the version actually
 * running on this device (see docs/hardware.md), silently fails to parse
 * that syntax with no console error, confirmed the hard way in the
 * sibling dashboard/ and ui/ prototypes. This file is a non-module
 * classic <script>, same reason as those two: a file:// page load has no
 * Content-Type header, so a module script tag fails to load at all.
 *
 * This is NOT a real terminal emulator. It is a minimal line-buffer
 * renderer over the exact same WebSocket JSON protocol a real xterm.js
 * integration would use (documented in docs/terminal.md and
 * terminal/frontend/vendor/README.md), so the agent side needs zero
 * changes when xterm.js is vendored in a future session with network
 * access.
 */

// terminal/agent/terminal-agent.py's own WebSocket, PTY I/O only.
const AGENT_WS_URL = 'ws://localhost:8766';
// The EXISTING bridge/ daemon's WebSocket (hardware buttons + knob),
// listened to read-only here for session-chrome control. Deliberately
// UNTOUCHED on the daemon side -- see docs/terminal.md for why button
// code -> action mapping is done here, client-side, instead of in bridge/.
const BRIDGE_WS_URL = 'ws://localhost:8765';

// Raw evdev KEY_* codes are UNCONFIRMED for this unit (see
// docs/research/hardware-interaction.md "Open item carried over"). These
// are placeholders, not verified mappings; replace once a live capture
// identifies the real codes for buttons 1-5.
const BUTTON_ACTIONS = {
  1: 'next_session',
  2: 'prev_session',
  3: 'interrupt',
  4: 'clear',
  5: 'respawn',
};
const KNOB_SCROLL_LINES_PER_STEP = 2;

const state = {
  sessions: [],       // [{index, name}]
  buffers: {},         // index -> array of output chunks (strings)
  activeSession: 0,
  agentSocket: null,
  agentReady: false,
};

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
  // Cap retained text so a long-running session doesn't grow this page's
  // memory unbounded; this is a placeholder renderer, not real scrollback.
  const MAX_CHUNKS = 2000;
  if (state.buffers[sessionIndex].length > MAX_CHUNKS) {
    state.buffers[sessionIndex].splice(0, state.buffers[sessionIndex].length - MAX_CHUNKS);
  }
  if (sessionIndex === state.activeSession) {
    renderScreen();
  }
}

function renderScreen() {
  const screen = document.getElementById('screen');
  const chunks = state.buffers[state.activeSession] || [];
  const wasAtBottom = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 4;
  screen.textContent = chunks.join('');
  if (wasAtBottom) {
    screen.scrollTop = screen.scrollHeight;
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
      appendOutput(msg.session, '\n[session exited, code=' + msg.code + ' -- use button 5 to respawn]\n');
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
      handleButtonAction(BUTTON_ACTIONS[msg.code]);
    }
  };
}

function scrollActiveScreen(delta) {
  // Scrollback lives entirely client-side in this placeholder renderer
  // (the full text is already in the DOM), so knob-scroll never touches
  // the agent or the bridge daemon -- purely a local DOM scroll.
  const screen = document.getElementById('screen');
  const lineHeight = 17; // approx, matches .screen's font-size/line-height
  screen.scrollTop += delta * KNOB_SCROLL_LINES_PER_STEP * lineHeight;
}

function handleButtonAction(action) {
  if (!action) return;
  const sessionIndexes = state.sessions.map(function (s) { return s.index; });
  if (action === 'next_session' || action === 'prev_session') {
    if (sessionIndexes.length === 0) return;
    const pos = sessionIndexes.indexOf(state.activeSession);
    const step = action === 'next_session' ? 1 : -1;
    const nextPos = (pos + step + sessionIndexes.length) % sessionIndexes.length;
    state.activeSession = sessionIndexes[nextPos];
    renderTabs();
    renderScreen();
    sendAgent({ type: 'resize', session: state.activeSession, cols: 80, rows: 24 });
  } else if (action === 'interrupt' || action === 'clear' || action === 'respawn') {
    sendAgent({ type: 'control', session: state.activeSession, action: action });
  }
}

// --- Keyboard input -------------------------------------------------------
// Per docs/research/bt-hid-terminal-feasibility.md's design conclusion: a
// paired BT keyboard surfaces as a normal evdev node that Chromium's own
// native input stack turns into ordinary keydown/keyup DOM events,
// exactly like the touchscreen already bypasses the bridge daemon. So
// this is a completely standard browser keydown handler, no new evdev
// code needed here at all.

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

function main() {
  document.getElementById('screen').addEventListener('keydown', onKeyDown);
  document.getElementById('screen').focus();
  connectAgent();
  connectBridge();
}

main();
