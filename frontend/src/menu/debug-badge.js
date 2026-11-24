/*
 * Persistent DEBUG badge (docs/research/debug-toggles-plan.md section 6,
 * task T-D4.2): one generic indicator, visible on every screen including the
 * terminal, while either ADB or UART is enabled. Lives in the status bar
 * (index.html), outside the menu overlay, so it survives menu navigation and
 * the terminal view untouched; toggled with the `hidden` class only, never a
 * style write, so it never reserves space while off. Not shown while the
 * first-boot overlay is active (`blocked`, passed in by boot.js).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.debugBadge).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Sel = node ? require('../state/selectors.js') : TR.selectors;

  function create(doc) {
    return {
      render: function (domain, blocked) {
        var el = doc.getElementById('debug-badge');
        var show = !blocked && Sel.deviceDebugActive(domain);
        el.classList.toggle('hidden', !show);
      },
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.debugBadge = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
