/*
 * Injectable time source. Production code never calls setTimeout/Date.now
 * directly; it receives a `timers` object so node tests can drive time with
 * a fake. createNamedTimers adds the "one pending timer per name" pattern
 * the interaction effects use (esc-hold, enter-guard, toast): starting a
 * name replaces the pending one, and firing reports the name as an event.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.timers).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  // env: an object with setTimeout/clearTimeout/setInterval/clearInterval
  // (default: the global scope).
  function create(env) {
    var g = env || root;
    return {
      setTimeout: function (fn, ms) { return g.setTimeout(fn, ms); },
      clearTimeout: function (id) { g.clearTimeout(id); },
      setInterval: function (fn, ms) { return g.setInterval(fn, ms); },
      clearInterval: function (id) { g.clearInterval(id); },
      now: function () { return Date.now(); },
      // Monotonic: never steps backward or forward with the wall clock (NTP
      // sync, DST, a user setting the time). Durations (the admin unlock
      // countdown) must be anchored on this, never on now().
      mono: function () {
        return g.performance && typeof g.performance.now === 'function' ? g.performance.now() : Date.now();
      },
    };
  }

  function createNamedTimers(timers, onFire) {
    var pending = {};

    function cancel(name) {
      if (Object.prototype.hasOwnProperty.call(pending, name)) {
        timers.clearTimeout(pending[name]);
        delete pending[name];
      }
    }

    function start(name, ms) {
      cancel(name);
      pending[name] = timers.setTimeout(function () {
        delete pending[name];
        onFire(name);
      }, ms);
    }

    function has(name) {
      return Object.prototype.hasOwnProperty.call(pending, name);
    }

    function cancelAll() {
      Object.keys(pending).forEach(cancel);
    }

    return { start: start, cancel: cancel, has: has, cancelAll: cancelAll };
  }

  var api = { create: create, createNamedTimers: createNamedTimers };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.timers = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
