/*
 * Top-level Settings screen. Rows are data computed from the domain state;
 * the Wi-Fi row exists only while an adapter does (wifi_available), so an
 * unplugged adapter hides it live.
 *
 * Every screen module exports build(deps) -> {defs, reactions, initialUi},
 * deps = {WL, UL, Cmd, Sel, cfg}; menu/screens/index.js merges them.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensRoot).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function build(deps) {
    var Cmd = deps.Cmd;
    var b = deps.cfg.brightness;

    function brightnessRow(d) {
      var br = d.brightness;
      var max = br.max || b.fallback_max;
      return {
        id: 'brightness', label: 'Brightness', kind: 'range',
        value: br.value,
        display: br.value === null || !br.max ? '...' : Math.round((br.value / br.max) * 100) + '%',
        min: 0,
        max: max,
        step: Math.max(1, Math.round(max / b.steps)),
        change: function (v) { return [{ dispatch: { type: 'local/brightness', value: v } }, { send: Cmd.brightnessSet(v) }]; },
      };
    }

    var defs = {
      root: {
        title: 'Settings',
        rows: function (ctx) {
          var d = ctx.domain;
          var rows = [
            { id: 'bt', label: 'Bluetooth', kind: 'nav', target: 'bluetooth' },
            brightnessRow(d),
            { id: 'terminal', label: 'Terminal', kind: 'nav', target: 'terminal' },
            { id: 'timezone', label: 'Timezone: ' + (d.timezone.current || '...'), kind: 'nav', target: 'timezone-groups' },
            // Locked: the unlock gate first; with a live admin session: the list.
            { id: 'users', label: 'Users', kind: 'action', afterSelect: 'none', activate: [{ push: deps.Sel.isUnlocked(d) ? 'users-list' : 'users-unlock' }] },
          ];
          if (d.wifi.available) {
            var st = d.wifi.status;
            var suffix = st && st.state === 'connected' && st.ssid ? ': ' + st.ssid : '';
            rows.splice(1, 0, { id: 'wifi', label: 'Wi-Fi' + suffix, kind: 'nav', target: 'wifi' });
          }
          return rows;
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
    root.TR.screensRoot = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
