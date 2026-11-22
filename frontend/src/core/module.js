/*
 * Load-order guard for the classic-script modules. Every module file is a
 * dual-load wrapper: in the browser it attaches its API to the single global
 * namespace window.TR, under node it sets module.exports. Script order in
 * index.html is the only thing that makes a dependency available in the
 * browser, so boot.js calls TR.module.assertLoaded([...]) once to turn a
 * wrong order into one clear error instead of a later "undefined" crash.
 *
 * Plain ES2018 subset, classic script.
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  // names: array of TR property names; ns: the namespace object (default window.TR).
  function assertLoaded(names, ns) {
    var TR = ns || root.TR || {};
    var missing = names.filter(function (n) { return !TR[n]; });
    if (missing.length) {
      throw new Error('TR modules missing: ' + missing.join(', ') + ' (check the <script> order in index.html)');
    }
  }

  function loaded(ns) {
    return Object.keys(ns || root.TR || {}).sort();
  }

  var api = { assertLoaded: assertLoaded, loaded: loaded };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.module = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
