/*
 * Terminal settings: font size (live range) and font family.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensTerminal).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function build(deps) {
    var ui = deps.cfg.ui;
    var families = ui.font_families;

    var defs = {
      terminal: {
        title: 'Terminal',
        rows: function (ctx) {
          var t = ctx.domain.terminal;
          return [
            {
              id: 'font-size', label: 'Font size', kind: 'range',
              value: t.fontSize, display: String(t.fontSize),
              min: ui.font_size_min, max: ui.font_size_max, step: ui.font_size_step,
              change: function (v) {
                return [{ dispatch: { type: 'local/terminal', patch: { fontSize: v } } }, { term: { fontSize: v } }];
              },
            },
            { id: 'font-family', label: 'Font family: ' + families[t.fontFamilyIndex], kind: 'nav', target: 'font-family-list' },
          ];
        },
      },
      'font-family-list': {
        title: 'Font family',
        rows: function () {
          return families.map(function (f, i) {
            return {
              id: 'font-' + i, label: f, kind: 'action', afterSelect: 'pop',
              activate: [
                { dispatch: { type: 'local/terminal', patch: { fontFamilyIndex: i } } },
                { term: { fontFamily: "'" + f + "', monospace" } },
              ],
            };
          });
        },
      },
    };

    return { defs: defs, reactions: {}, initialUi: function () { return {}; } };
  }

  var api = { build: build };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensTerminal = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
