/*
 * Typed layer over the terminal-agent channel (docs/terminal.md protocol,
 * unchanged). Decodes frames to app events and builds the outgoing frames,
 * so no call site writes a JSON literal. Pure apart from the channel it is
 * given.
 *
 * Decoded events: session_list, output, exited, clock_sync. Unknown types
 * are logged at debug level (never silently swallowed).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.agentClient).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  var KNOWN = { session_list: 1, output: 1, exited: 1, clock_sync: 1 };

  // Pure frame builders (client -> agent).
  var commands = {
    input: function (session, data) { return { type: 'input', session: session, data: data }; },
    resize: function (session, cols, rows) { return { type: 'resize', session: session, cols: cols, rows: rows }; },
    control: function (session, action) { return { type: 'control', session: session, action: action }; },
  };

  // Frame (agent -> client) to app event, or null when not understood.
  function decode(msg) {
    if (!Object.prototype.hasOwnProperty.call(KNOWN, msg.type)) return null;
    switch (msg.type) {
      case 'session_list':
        return { type: 'session_list', sessions: msg.sessions || [] };
      case 'output':
        return { type: 'output', session: msg.session, data: msg.data };
      case 'exited':
        return {
          type: 'exited', session: msg.session, code: msg.code,
          needs_manual_respawn: !!msg.needs_manual_respawn, auto_respawning: !!msg.auto_respawning,
        };
      default:
        return { type: 'clock_sync', offset_ms: msg.offset_ms, status: msg.status };
    }
  }

  // channel: transport/channel.js instance; emit(event) receives decoded events.
  function create(channel, emit, logger) {
    var log = (logger || Logging).child('agent');
    channel.onMessage(function (msg) {
      var ev = decode(msg);
      if (!ev) {
        log.debug('ignoring unknown message type', msg.type);
        return;
      }
      emit(ev);
    });
    return {
      sendInput: function (session, data) { return channel.send(commands.input(session, data)); },
      // Self-healing (the next open re-fits), so a drop needs no user feedback.
      sendResize: function (session, cols, rows) { return channel.send(commands.resize(session, cols, rows), { quiet: true }); },
      sendControl: function (session, action) { return channel.send(commands.control(session, action)); },
    };
  }

  var api = { create: create, decode: decode, commands: commands };

  if (node) module.exports = api;
  else TR.agentClient = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
