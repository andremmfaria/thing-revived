/*
 * Tiny store for DOMAIN data only: what the agents told us (sessions,
 * connection status, clock, Bluetooth, Wi-Fi, brightness, timezone) plus the
 * two terminal settings. Terminal output bytes are NOT here (xterm plus one
 * byte-capped replay buffer per session own them) and menu navigation lives
 * in the interaction state machine.
 *
 * dispatch() runs the reducer synchronously and notifies subscribers only
 * when the state reference changed, so unknown actions cost nothing.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.store).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function create(reducer, initial) {
    var state = initial;
    var subscribers = [];

    function getState() { return state; }

    function dispatch(action) {
      var next = reducer(state, action);
      if (next === state) return state;
      state = next;
      subscribers.slice().forEach(function (fn) { fn(state, action); });
      return state;
    }

    function subscribe(fn) {
      subscribers.push(fn);
      return function unsubscribe() {
        var i = subscribers.indexOf(fn);
        if (i !== -1) subscribers.splice(i, 1);
      };
    }

    return { getState: getState, dispatch: dispatch, subscribe: subscribe };
  }

  var api = { create: create };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.store = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
