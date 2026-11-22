/*
 * Hand-rolled, table-driven statechart runner (no library). A definition is
 *
 *   { states: { '<path>': { '<event type>': handler } } }
 *
 * where a path is dotted ('open.pairing.confirm'). transition(state, event)
 * looks the handler up on state.mode, then on each ancestor path
 * ('open.pairing', 'open', then the root ''), so shared behavior lives on the
 * parent and children override only what differs. A handler is
 * handler(state, event, machine) and returns {state, effects} to consume the
 * event, or null/undefined to let the parent try. The function is pure: it
 * never mutates `state`, and effects are plain data for an interpreter to
 * execute (timers included; their firing comes back in as an event).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.machine).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  // 'open.pairing.confirm' -> ['open.pairing.confirm', 'open.pairing', 'open', '']
  function chain(mode) {
    var parts = mode.split('.');
    var out = [];
    for (var n = parts.length; n > 0; n--) out.push(parts.slice(0, n).join('.'));
    out.push('');
    return out;
  }

  function create(def) {
    var machine = {
      chain: chain,
      transition: function (state, event) {
        var paths = chain(state.mode);
        for (var i = 0; i < paths.length; i++) {
          var table = def.states[paths[i]];
          var handler = table && Object.prototype.hasOwnProperty.call(table, event.type) ? table[event.type] : null;
          if (!handler) continue;
          var r = handler(state, event, machine);
          if (r) return { state: r.state || state, effects: r.effects || [] };
        }
        return { state: state, effects: [] };
      },
    };
    return machine;
  }

  var api = { create: create, chain: chain };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.machine = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
