/*
 * Typed layer over the terminal agent channel (config/protocol/terminal fixtures,
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
  var Types = node ? require('../utils/types.js') : TR.types;
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  // Pure frame builders (client -> agent), catalogued in utils/types.js.
  var commands = Types.agentCommands;

  // Frame (agent -> client) to app event, or null when not understood.
  function decode(msg) {
    if (!Types.isAgentEvent(msg)) return null;
    switch (msg.type) {
      case Types.AgentEvent.session_list:
        return { type: Types.AgentEvent.session_list, sessions: msg.sessions || [] };
      case Types.AgentEvent.output:
        return { type: Types.AgentEvent.output, session: msg.session, data: msg.data };
      case Types.AgentEvent.exited:
        return {
          type: 'exited', session: msg.session, code: msg.code,
          needs_manual_respawn: !!msg.needs_manual_respawn, auto_respawning: !!msg.auto_respawning,
        };
      default:
        return { type: Types.AgentEvent.clock_sync, offset_ms: msg.offset_ms, status: msg.status };
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
