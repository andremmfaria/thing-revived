/*
 * Debug access settings screen (docs/research/debug-toggles-plan.md sections
 * 5.3, 6 and 9, tasks T-D4.1/T-D4.2): runtime ADB and UART root console
 * toggles. Behind the Users admin unlock (reuses users-kit.js's screen()
 * wrapper and lockedRows(), the same gate as the Users section; the server
 * enforces it too, this is only a convenience). Each surface is one
 * hold-to-confirm row (`hold: true`, like usb-role.js's Reboot now row) that
 * sends debug_set with the surface's fixed confirm string, never typed.
 * State shown is always the last debug_state event, never optimistic: a
 * toggle does not flip the label until the controller confirms it. Errors
 * from debug_error are a persistent note on this screen (not a timed toast),
 * cleared on entering or leaving the screen.
 *
 * Wire naming (config/protocol/controller/README.md): the controller task
 * shipped `debug_get`/`debug_set` and `debug_state`/`debug_error` with a
 * `surface` field, not the plan text's `debug_access_*` names and `kind`
 * field; this screen follows the shipped names. `debug_state.blocked` names
 * why ADB is unavailable (today only `usb_role_host`) rather than this
 * screen deriving it from the USB role itself.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensDebugAccess).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Kit = node ? require('./users-kit.js') : TR.screensUsersKit;
  var DL = node ? require('../../debug-access/debug-access-logic.js') : TR.debugAccessLogic;

  var SCREEN = 'debug-access';

  function build(deps) {
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;
    var k = Kit.create(deps);

    function clearError() { return [{ dispatch: { type: 'local/debug_clear_error' } }]; }

    // One surface's state line plus its hold-to-confirm toggle, or (ADB only,
    // while debug_state.blocked names a reason) a disabled label instead.
    function surfaceRows(surface, label, enabled, buildDefault, blockReason) {
      var state = enabled === null ? '...' : (enabled ? 'ON' : 'OFF');
      var buildState = buildDefault ? 'ON' : 'OFF';
      var rows = [{ id: 'debug-' + surface + '-state', label: label + ': ' + state + ' (build default: ' + buildState + ')', kind: 'label' }];
      if (blockReason) {
        rows.push({ id: 'debug-' + surface + '-blocked', label: label + ': switch USB mode to Device to use it', kind: 'label' });
        return rows;
      }
      var next = enabled === true ? false : true;
      rows.push({
        id: 'debug-' + surface + '-toggle', label: (next ? 'Enable ' : 'Disable ') + label, kind: 'action', afterSelect: 'none', hold: true,
        activate: [{ send: Cmd.debugSet(surface, next, DL.CONFIRM[surface]) }].concat(clearError()),
      });
      return rows;
    }

    var defs = {};
    defs[SCREEN] = k.screen({
      title: 'Debug access',
      hint: { text: 'runtime only, resets on reboot \\u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return [{ send: Cmd.debugGet() }].concat(clearError()); },
      onLeave: function () { return clearError(); },
      rows: function (ctx) {
        var d = ctx.domain.debugAccess;
        var blocked = Sel.debugAccessAdbBlocked(ctx.domain);
        var rows = surfaceRows('adb', 'ADB', d.adb, d.adbBuildDefault, blocked).concat(surfaceRows('uart', 'UART console', d.uart, d.uartBuildDefault, false));
        rows.push(k.note('debug-warn-1', 'ADB gives an unauthenticated root shell over USB, no password.'));
        rows.push(k.note('debug-warn-2', 'UART gives a passwordless root shell over the serial console.'));
        rows.push(k.note('debug-warn-3', 'A toggle can drop the USB link for about 10 seconds.'));
        if (d.error) rows.push(k.note('debug-error', DL.errorText(d.error.code, d.error.message)));
        return rows;
      },
    }, { open: false });

    return { defs: defs, reactions: {}, initialUi: function () { return {}; } };
  }

  var api = { build: build, SCREEN: SCREEN };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensDebugAccess = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
