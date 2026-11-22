/*
 * Bluetooth screens: power, pairing list, paired devices and their actions,
 * and the two pairing prompts (confirm and passkey display), plus the
 * controller-event reactions that open/close them.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensBluetooth).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function build(deps) {
    var Cmd = deps.Cmd;

    var CONFIRM_HINT = { text: 'knob: move \u00b7 esc: confirm selection \u00b7 esc (hold): cancel pairing', pinned: true };
    var PASSKEY_HINT = { text: 'esc (hold): cancel pairing', pinned: true };

    function deviceLabel(d) { return d.name || d.address; }

    var defs = {
      bluetooth: {
        title: 'Bluetooth',
        rows: function (ctx) {
          var powered = ctx.domain.bt.powered;
          return [
            {
              id: 'bt-power', label: 'Power', kind: 'toggle',
              display: powered === null ? '...' : (powered ? 'ON' : 'OFF'),
              activate: [{ send: Cmd.btSetPower(!powered) }],
            },
            {
              id: 'bt-pair', label: 'Pair a device...', kind: 'nav', target: 'bt-pair-list',
              // The pre-refactor row also carried an onLeave that sent
              // bt_scan_stop, but the old navigation never invoked row-level
              // onLeave, so the scan was never stopped from here. Behavior is
              // kept as it was; see the migration notes.
              enter: [{ dispatch: { type: 'local/bt_devices_reset' } }, { send: Cmd.btScanStart() }],
            },
            { id: 'bt-paired', label: 'Paired devices', kind: 'nav', target: 'bt-paired-list', enter: [{ send: Cmd.btPairedListGet() }] },
          ];
        },
      },
      'bt-paired-list': {
        title: 'Paired devices',
        hint: 'knob: move \u00b7 esc: open device \u00b7 esc (hold): back',
        rows: function (ctx) {
          var paired = ctx.domain.bt.paired;
          if (paired === null) return [{ id: 'bt-paired-loading', label: 'Loading...', kind: 'label' }];
          if (paired.length === 0) return [{ id: 'bt-paired-empty', label: 'No paired devices', kind: 'label' }];
          return paired.map(function (d) {
            return {
              id: 'bt-paired-' + d.address, label: deviceLabel(d), kind: 'nav-dynamic',
              target: 'bt-paired-actions', statusDot: d.connected,
              enter: [{ ui: { bt: { selectedPaired: d } } }],
            };
          });
        },
      },
      'bt-paired-actions': {
        title: function (ctx) {
          var d = ctx.ui.bt.selectedPaired;
          return d ? deviceLabel(d) : 'Device';
        },
        hint: 'knob: move \u00b7 esc: confirm action \u00b7 esc (hold): back',
        rows: function (ctx) {
          var d = ctx.ui.bt.selectedPaired;
          if (!d) return [];
          var rows = [];
          if (d.connected) {
            rows.push({ id: 'bt-paired-disconnect', label: 'Disconnect', kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.btDisconnect(d.address) }] });
          } else {
            rows.push({ id: 'bt-paired-connect', label: 'Connect', kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.btConnect(d.address) }] });
          }
          rows.push({ id: 'bt-paired-forget', label: 'Forget', kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.btForget(d.address) }] });
          return rows;
        },
      },
      'bt-pair-list': {
        title: 'Pair a device',
        rows: function (ctx) {
          var devices = ctx.domain.bt.devices;
          if (devices.length === 0) return [{ id: 'bt-pair-scanning', label: 'Scanning...', kind: 'label' }];
          return devices.map(function (d) {
            return {
              id: 'bt-dev-' + d.address, label: deviceLabel(d), kind: 'action',
              afterSelect: 'none', // wait for bt_pairing_confirm_request / bt_pair_result
              // A second bt_pair while one is in flight makes BlueZ answer
              // InProgress and can starve the first attempt's auth window.
              activate: ctx.ui.bt.pairing ? [] : [{ send: Cmd.btPair(d.address) }],
            };
          });
        },
      },
      'bt-confirm': {
        mode: 'open.pairing.confirm',
        hint: CONFIRM_HINT,
        title: function (ctx) {
          var p = ctx.ui.bt.pairing;
          return p ? ('Pair with ' + p.address + '? (' + p.passkey + ')') : 'Pairing';
        },
        rows: function () {
          return [
            { id: 'bt-confirm-yes', label: 'Accept', kind: 'action', afterSelect: 'pop', activate: [{ send: Cmd.btPairingConfirm(true) }] },
            { id: 'bt-confirm-no', label: 'Deny', kind: 'action', afterSelect: 'pop', activate: [{ send: Cmd.btPairingConfirm(false) }] },
          ];
        },
        // Manual cancel (ESC hold) never gets a bt_pair_result, so clear the
        // in-flight marker here too or later attempts stay blocked.
        onLeave: function () { return [{ ui: { bt: { pairing: null } } }]; },
      },
      'bt-passkey-display': {
        mode: 'open.pairing.passkey',
        hint: PASSKEY_HINT,
        // No choice here: the user types the passkey on the keyboard and
        // presses Enter, so the screen just has to stay up long enough.
        title: function (ctx) {
          var p = ctx.ui.bt.pairing;
          return p ? ('Pair with ' + p.address) : 'Pairing';
        },
        rows: function (ctx) {
          var p = ctx.ui.bt.pairing;
          return [
            { id: 'bt-passkey-value', kind: 'label', label: p ? ('Type ' + p.passkey + ' on the keyboard, then press Enter') : 'Type the passkey on the keyboard' },
            { id: 'bt-passkey-waiting', kind: 'label', label: 'Waiting for device...' },
          ];
        },
        onLeave: function () { return [{ ui: { bt: { pairing: null } } }]; },
      },
    };

    function isPairingScreen(id) { return id === 'bt-confirm' || id === 'bt-passkey-display'; }

    var reactions = {
      bt_error: function (ctx, msg) { return [{ toast: 'BT error (' + msg.context + '): ' + msg.message }]; },
      settings_error: function (ctx, msg) { return [{ toast: 'Settings error (' + msg.context + '): ' + msg.message }]; },
      // Keep the device action screen in sync with a proactive list update
      // (after connect/disconnect, or implicitly after forget).
      bt_paired_list: function (ctx) {
        var sel = ctx.ui.bt.selectedPaired;
        if (ctx.top !== 'bt-paired-actions' || !sel) return [];
        var still = (ctx.domain.bt.paired || []).filter(function (d) { return d.address === sel.address; })[0];
        if (still) return [{ ui: { bt: { selectedPaired: still } } }];
        return [{ pop: true }]; // forgotten: nothing left to act on
      },
      bt_pairing_confirm_request: function (ctx, msg) {
        return [
          { ui: { bt: { pairing: { address: msg.address, passkey: msg.passkey, kind: 'confirm' } } } },
          { open: true },
          { push: 'bt-confirm' },
        ];
      },
      bt_pairing_passkey_display: function (ctx, msg) {
        return [
          { ui: { bt: { pairing: { address: msg.address, passkey: msg.passkey, kind: 'display' } } } },
          { open: true },
          { push: 'bt-passkey-display' },
        ];
      },
      // Attempt concluded: clear the in-flight marker and leave the prompt.
      bt_pair_result: function (ctx, msg) {
        var cmds = [
          { toast: msg.success ? ('Paired with ' + msg.address) : ('Pairing failed: ' + (msg.error || 'unknown error')) },
          { ui: { bt: { pairing: null } } },
        ];
        if (isPairingScreen(ctx.top)) cmds.push({ pop: true });
        return cmds;
      },
    };

    return {
      defs: defs,
      reactions: reactions,
      initialUi: function () { return { bt: { selectedPaired: null, pairing: null } }; },
    };
  }

  var api = { build: build };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensBluetooth = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
