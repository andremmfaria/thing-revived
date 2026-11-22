/*
 * Terminal sessions. Two parts:
 *  - createReplayBuffers: one byte-capped replay buffer per session. xterm
 *    owns the live bytes; this only keeps enough recent output to repaint a
 *    tab when it is selected again. Capped by bytes (UTF-8 length), not chunk
 *    count, because a chatty curses app can send very large chunks and four
 *    sessions share 512 MB with Chromium. Oldest chunks go first; a single
 *    chunk larger than the cap is cut to its tail.
 *  - createSessions: tab switching and output routing over the store, the
 *    replay buffers, the terminal view and the agent client.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.sessions).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Sel = node ? require('../state/selectors.js') : TR.selectors;

  // UTF-8 byte length without allocating (TextEncoder is not worth it here).
  function utf8Length(s) {
    var n = 0;
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c < 0x80) n += 1;
      else if (c < 0x800) n += 2;
      else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) { n += 4; i++; }
      else n += 3;
    }
    return n;
  }

  function tailWithin(s, cap) {
    var cut = s.slice(Math.max(0, s.length - cap));
    while (cut.length > 0 && utf8Length(cut) > cap) cut = cut.slice(1);
    // never start in the middle of a surrogate pair
    if (cut.length > 0 && cut.charCodeAt(0) >= 0xdc00 && cut.charCodeAt(0) <= 0xdfff) cut = cut.slice(1);
    return cut;
  }

  function createReplayBuffers(capBytes) {
    var buffers = {}; // index -> {chunks, bytes}

    function append(index, data) {
      var b = buffers[index] || (buffers[index] = { chunks: [], bytes: 0 });
      var len = utf8Length(data);
      if (len > capBytes) {
        data = tailWithin(data, capBytes);
        len = utf8Length(data);
      }
      b.chunks.push(data);
      b.bytes += len;
      while (b.bytes > capBytes && b.chunks.length > 1) {
        b.bytes -= utf8Length(b.chunks.shift());
      }
    }

    return {
      append: append,
      get: function (index) { return buffers[index] ? buffers[index].chunks.slice() : []; },
      bytes: function (index) { return buffers[index] ? buffers[index].bytes : 0; },
      cap: capBytes,
    };
  }

  // deps: { store, replay, termView:{write,replay,fitAndResize}, agent:{sendControl} }
  function createSessions(deps) {
    var store = deps.store;

    function onOutput(index, data) {
      deps.replay.append(index, data);
      // Only the active session is written live; xterm interprets the
      // ANSI/VT100 sequences curses programs need.
      if (index === store.getState().sessions.active) deps.termView.write(data);
    }

    function onSessionList(sessions) {
      store.dispatch({ type: 'session_list', sessions: sessions });
      deps.termView.replay(deps.replay.get(store.getState().sessions.active));
    }

    function onExited(msg) {
      store.dispatch({ type: 'exited', session: msg.session, needs_manual_respawn: msg.needs_manual_respawn });
      if (msg.needs_manual_respawn) {
        onOutput(msg.session, '\n[session exited -- select this tab again to restart it]\n');
      } else if (msg.auto_respawning) {
        onOutput(msg.session, '\n[session exited, code=' + msg.code + ' -- restarting...]\n');
      } else {
        onOutput(msg.session, '\n[session exited, code=' + msg.code + ']\n');
      }
    }

    // Direct session switch. Selecting a tab already known dead also asks the
    // agent to respawn it: no extra hardware trigger or message type needed.
    function switchTo(index) {
      var d = store.getState();
      if (!Sel.sessionExists(d, index)) return; // not configured: no-op, not a crash
      if (d.sessions.dead[index]) {
        deps.agent.sendControl(index, 'respawn');
        store.dispatch({ type: 'local/clearDead', index: index });
      }
      if (index === store.getState().sessions.active) return;
      store.dispatch({ type: 'local/active', index: index });
      deps.termView.replay(deps.replay.get(index));
      // Forces a resize message to the new session even when the computed
      // size is unchanged: the agent only SIGWINCHes on a size change, and a
      // freshly switched shell must redraw its prompt.
      deps.termView.fitAndResize();
    }

    return { onOutput: onOutput, onSessionList: onSessionList, onExited: onExited, switchTo: switchTo };
  }

  var api = { createReplayBuffers: createReplayBuffers, createSessions: createSessions, utf8Length: utf8Length };

  if (node) module.exports = api;
  else TR.sessions = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
