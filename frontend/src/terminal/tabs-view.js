/*
 * Tab bar, session label and the two connection indicators in the status
 * bar. render(domain) is idempotent; it rebuilds the tab bar (a handful of
 * nodes) and flips the status dots.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.tabsView).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Sel = node ? require('../state/selectors.js') : TR.selectors;

  function create(doc) {
    var el = function (id) { return doc.getElementById(id); };

    function setDot(dotId, textId, ok, label) {
      var dot = el(dotId);
      if (ok) dot.classList.add('ok'); else dot.classList.remove('ok');
      el(textId).textContent = label;
    }

    return {
      render: function (domain) {
        var bar = el('tab-bar');
        bar.innerHTML = '';
        Sel.tabs(domain).forEach(function (t) {
          var tab = doc.createElement('div');
          tab.className = 'tab' + (t.active ? ' active' : '');
          tab.textContent = t.name;
          bar.appendChild(tab);
        });
        el('session-text').textContent = Sel.sessionLabel(domain);
      },
      renderConnection: function (domain) {
        var a = Sel.connection(domain, 'agent');
        setDot('agent-dot', 'agent-text', a.ok, a.label);
        var b = Sel.connection(domain, 'bridge');
        setDot('bridge-dot', 'bridge-text', b.ok, b.label);
      },
      setDot: setDot,
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.tabsView = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
