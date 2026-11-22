/*
 * Wi-Fi screens (docs/wifi.md): status home, scan list, password entry (real
 * <input> or knob character picker), connecting/result, saved networks and
 * forget confirmation, plus the controller-event reactions. All Wi-Fi
 * knowledge lives in this file and in wifi/wifi-logic.js; the generic menu
 * code only knows `textField`, `custom`, `onKnob`/`onSelect`/`onCustomTap`.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensWifi).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../../utils/types.js') : TR.types;

  function build(deps) {
    var WL = deps.WL;
    var Cmd = deps.Cmd;

    function connectSaved(ssid) {
      return [
        { ui: { wifi: { result: null, connecting: ssid } } },
        { send: Cmd.wifiConnectSaved(ssid) },
        { push: 'wifi-connecting' },
      ];
    }

    // What choosing a scan result does: toast, connect with stored
    // credentials, connect open, or ask for the password.
    function selectNetwork(ctx, n) {
      if (!n.supported) return [{ toast: WL.securityLabel(n.security) + ' networks are not supported here' }];
      if (WL.findSaved(ctx.domain.wifi.saved, n.ssid) !== -1) {
        // Saved: reuse the stored credentials. A changed password means
        // forgetting it first (Saved networks).
        return [{ ui: { wifi: { selected: n, result: null, connecting: n.ssid } } }, { send: Cmd.wifiConnectSaved(n.ssid) }, { push: 'wifi-connecting' }];
      }
      if (n.security === 'open') {
        return [{ ui: { wifi: { selected: n, result: null, connecting: n.ssid } } }, { send: Cmd.wifiConnect(n.ssid, '') }, { push: 'wifi-connecting' }];
      }
      return [{ ui: { wifi: { selected: n, result: null, password: '', showPassword: false } } }, { push: 'wifi-password' }];
    }

    function submitPassword(ctx) {
      var w = ctx.ui.wifi;
      if (!w.selected) return [];
      var err = WL.validatePassword(w.password, w.selected.security);
      if (err) return [{ toast: err }];
      return [
        { ui: { wifi: { connecting: w.selected.ssid, result: null } } },
        { send: Cmd.wifiConnect(w.selected.ssid, w.password) },
        { push: 'wifi-connecting' },
      ];
    }

    function pickerSelect(ctx) {
      var w = ctx.ui.wifi;
      var r = WL.pickerApply(w.password, w.pickerIdx);
      var cmds = [{ ui: { wifi: { password: r.password } } }];
      if (r.done) {
        // Land on Connect so one more push submits.
        cmds.push({ pop: true }, { highlight: { 'wifi-password': 3 } });
      }
      return cmds;
    }

    var defs = {
      wifi: {
        title: 'Wi-Fi',
        onEnter: function () { return [{ send: Cmd.wifiGet() }, { send: Cmd.wifiSavedListGet() }]; },
        rows: function (ctx) {
          var w = ctx.domain.wifi;
          var rows = WL.statusRows(w.status).map(function (r, i) {
            return { id: 'wifi-st-' + i, label: r.label + ': ' + r.value, kind: 'label', bars: r.bars };
          });
          rows.push({
            id: 'wifi-scan', label: 'Scan for networks...', kind: 'nav', target: 'wifi-list',
            enter: [{ dispatch: { type: 'local/wifi_networks_reset' } }, { send: Cmd.wifiScan() }],
          });
          rows.push({ id: 'wifi-saved', label: 'Saved networks', kind: 'nav', target: 'wifi-saved', enter: [{ send: Cmd.wifiSavedListGet() }] });
          if (w.status && w.status.state !== 'disconnected') {
            rows.push({ id: 'wifi-disconnect', label: 'Disconnect', kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.wifiDisconnect() }] });
          }
          return rows;
        },
      },
      'wifi-list': {
        title: 'Wi-Fi networks',
        rows: function (ctx) {
          var w = ctx.domain.wifi;
          var rows = [];
          if (w.networks === null) rows.push({ id: 'wifi-scanning', label: 'Scanning...', kind: 'label' });
          else if (w.networks.length === 0) rows.push({ id: 'wifi-none', label: 'No networks found', kind: 'label' });
          (w.networks || []).forEach(function (n) {
            var saved = WL.findSaved(w.saved, n.ssid) !== -1;
            rows.push({
              id: 'wifi-net-' + n.ssid, label: n.ssid, kind: 'action', afterSelect: 'none',
              valueText: WL.securityLabel(n.security) + (saved ? ' (saved)' : ''),
              bars: WL.signalBars(n.signal_pct),
              activate: selectNetwork(ctx, n),
            });
          });
          rows.push({
            id: 'wifi-rescan', label: w.scanning ? 'Scanning...' : 'Rescan', kind: 'action', afterSelect: 'none',
            activate: w.scanning ? [] : [{ send: Cmd.wifiScan() }],
          });
          return rows;
        },
      },
      'wifi-password': {
        title: function (ctx) { return ctx.ui.wifi.selected ? ctx.ui.wifi.selected.ssid : 'Password'; },
        hint: 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back (discards password)',
        // Typing goes through the real <input> (on-screen keyboard on focus,
        // a Bluetooth keyboard types into it like any page input).
        textField: {
          hint: 'type the password \u00b7 enter or esc: done',
          value: function (ctx) { return ctx.ui.wifi.password; },
          masked: function (ctx) { return !ctx.ui.wifi.showPassword; },
          onInput: function (ctx, value) { return [{ ui: { wifi: { password: value } } }]; },
        },
        rows: function (ctx) {
          var w = ctx.ui.wifi;
          return [
            {
              id: 'wifi-pw-field', label: 'Password', kind: 'field', afterSelect: 'none',
              valueText: w.password.length ? WL.maskPassword(w.password, w.showPassword) : '(empty)',
              activate: [{ textEdit: 'start' }],
            },
            {
              id: 'wifi-pw-show', label: 'Show password', kind: 'toggle', display: w.showPassword ? 'ON' : 'OFF',
              activate: [{ ui: { wifi: { showPassword: !w.showPassword } } }],
            },
            { id: 'wifi-pw-picker', label: 'Pick characters with knob...', kind: 'nav', target: 'wifi-picker' },
            { id: 'wifi-pw-connect', label: 'Connect', kind: 'action', afterSelect: 'none', activate: submitPassword(ctx) },
          ];
        },
        // Leaving this screen (not just covering it with the picker or the
        // connecting screen) is what discards the typed password.
        onLeave: function () {
          return [{ ui: { wifi: { password: '', showPassword: false } } }, { text: 'reset' }];
        },
      },
      'wifi-picker': {
        title: 'Pick characters',
        mode: Types.Mode.PICKER,
        hint: 'knob: choose \u00b7 esc: add / apply \u00b7 esc (hold): back',
        custom: 'wifi-picker',
        onEnter: function () { return [{ ui: { wifi: { pickerIdx: WL.PICKER_START } } }]; },
        onKnob: function (ctx, delta) { return [{ ui: { wifi: { pickerIdx: WL.pickerMove(ctx.ui.wifi.pickerIdx, delta) } } }]; },
        onSelect: pickerSelect,
        // Touch: tap a neighbour to move there, tap the centre to apply it.
        onCustomTap: function (ctx, tap) {
          return tap.offset === 0 ? pickerSelect(ctx) : [{ ui: { wifi: { pickerIdx: tap.index } } }];
        },
      },
      'wifi-connecting': {
        title: function (ctx) {
          var w = ctx.ui.wifi;
          return w.connecting || (w.result ? w.result.ssid : 'Wi-Fi');
        },
        hint: 'esc (hold): back (connection continues in background)',
        rows: function (ctx) {
          var w = ctx.ui.wifi;
          if (w.connecting || !w.result) {
            return [{ id: 'wifi-conn-wait', label: 'Connecting to ' + (w.connecting || '...') + '...', kind: 'label' }];
          }
          if (w.result.success) {
            return [
              { id: 'wifi-conn-ok', label: 'Connected', kind: 'label' },
              { id: 'wifi-conn-done', label: 'Done', kind: 'action', afterSelect: 'none', activate: [{ popTo: 'wifi' }] },
            ];
          }
          return [
            { id: 'wifi-conn-err', label: w.result.message || 'Connection failed', kind: 'label' },
            { id: 'wifi-conn-back', label: 'Back', kind: 'action', afterSelect: 'none', activate: [{ pop: true }] },
          ];
        },
      },
      'wifi-saved': {
        title: 'Saved networks',
        rows: function (ctx) {
          var saved = ctx.domain.wifi.saved;
          if (saved === null) return [{ id: 'wifi-saved-loading', label: 'Loading...', kind: 'label' }];
          if (saved.length === 0) return [{ id: 'wifi-saved-empty', label: 'No saved networks', kind: 'label' }];
          return saved.map(function (n) {
            return {
              id: 'wifi-saved-' + n.ssid, label: n.ssid, kind: 'nav-dynamic', target: 'wifi-saved-actions',
              statusDot: n.current,
              enter: [{ ui: { wifi: { selectedSaved: n.ssid } } }],
            };
          });
        },
      },
      'wifi-saved-actions': {
        title: function (ctx) { return ctx.ui.wifi.selectedSaved || 'Network'; },
        rows: function (ctx) {
          var ssid = ctx.ui.wifi.selectedSaved;
          return [
            { id: 'wifi-saved-connect', label: 'Connect', kind: 'action', afterSelect: 'none', activate: connectSaved(ssid) },
            { id: 'wifi-saved-forget', label: 'Forget...', kind: 'nav', target: 'wifi-forget-confirm' },
          ];
        },
      },
      'wifi-forget-confirm': {
        title: function (ctx) { return 'Forget ' + (ctx.ui.wifi.selectedSaved || '') + '?'; },
        rows: function (ctx) {
          var ssid = ctx.ui.wifi.selectedSaved;
          return [
            // The wifi_saved_list that follows pops back to the list.
            { id: 'wifi-forget-yes', label: 'Forget this network', kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.wifiForget(ssid) }] },
            { id: 'wifi-forget-no', label: 'Cancel', kind: 'action', afterSelect: 'pop', activate: [] },
          ];
        },
      },
    };

    var reactions = {
      wifi_available: function (ctx, msg) {
        if (msg.available) return [];
        // Adapter unplugged: drop live state and leave any Wi-Fi screen.
        var cmds = [{ ui: { wifi: { connecting: null, password: '' } } }, { text: 'exit' }];
        if (ctx.stack.some(function (id) { return id.indexOf('wifi') === 0; })) cmds.push({ popToRoot: true });
        return cmds;
      },
      wifi_connecting: function (ctx, msg) { return [{ ui: { wifi: { connecting: msg.ssid, result: null } } }]; },
      wifi_connect_result: function (ctx, msg) {
        var wifi = { connecting: null, result: msg };
        if (msg.success) {
          wifi.password = '';
          wifi.showPassword = false;
        }
        // Land on the Back/Done action rather than the message label above it.
        var cmds = [{ ui: { wifi: wifi } }, { highlight: { 'wifi-connecting': 1 } }];
        if (ctx.top !== 'wifi-connecting') {
          // The user backed out while it was in flight: say how it ended.
          cmds.push({ toast: msg.success ? ('Connected to ' + msg.ssid) : ('Wi-Fi: ' + (msg.message || 'connection failed')) });
        }
        return cmds;
      },
      wifi_saved_list: function (ctx) {
        var sel = ctx.ui.wifi.selectedSaved;
        var gone = sel !== null && WL.findSaved(ctx.domain.wifi.saved, sel) === -1;
        // Forgotten (or removed): nothing left to act on, return to the list.
        if (gone && ctx.stack.indexOf('wifi-saved-actions') !== -1) return [{ popTo: 'wifi-saved' }];
        return [];
      },
      wifi_error: function (ctx, msg) { return [{ toast: 'Wi-Fi error (' + msg.context + '): ' + msg.message }]; },
    };

    return {
      defs: defs,
      reactions: reactions,
      initialUi: function () {
        return {
          wifi: {
            selected: null, selectedSaved: null, password: '', showPassword: false,
            pickerIdx: WL.PICKER_START, connecting: null, result: null,
          },
        };
      },
    };
  }

  var api = { build: build };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensWifi = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
