/*
 * Minimal pub/sub. Handlers run synchronously in subscription order. A
 * throwing handler does not stop the others; its error goes to opts.onError
 * (or is rethrown after all handlers ran when none is given).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.bus).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function create(opts) {
    var onError = (opts && opts.onError) || null;
    var handlers = {};

    function on(topic, fn) {
      (handlers[topic] = handlers[topic] || []).push(fn);
      return function off() {
        var list = handlers[topic] || [];
        var i = list.indexOf(fn);
        if (i !== -1) list.splice(i, 1);
      };
    }

    function once(topic, fn) {
      var off = on(topic, function (payload) {
        off();
        fn(payload);
      });
      return off;
    }

    function emit(topic, payload) {
      var list = (handlers[topic] || []).slice();
      var firstError = null;
      list.forEach(function (fn) {
        try {
          fn(payload);
        } catch (err) {
          if (onError) onError(topic, err);
          else if (!firstError) firstError = err;
        }
      });
      if (firstError) throw firstError;
    }

    function count(topic) {
      return (handlers[topic] || []).length;
    }

    return { on: on, once: once, emit: emit, count: count };
  }

  var api = { create: create };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.bus = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
