/*
 * Device settings root screen (docs/research/device-settings-plan.md section
 * 8.1): Unlock (or Lock now with a display countdown), Users, Groups, Reboot
 * device (hold-to-confirm) and USB mode. Every row after the first is
 * disabled (still highlightable) while the admin session is locked; the
 * server enforces the lock, this is only a convenience.
 *
 * The 1 s `ds-tick` timer only forces a re-render so the countdown moves; it
 * runs while this screen is on the stack (so also under USB mode) and stops
 * when it is left.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensDeviceSettings).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var DL = node ? require('../../device-settings/device-settings-logic.js') : TR.deviceSettingsLogic;

  var SCREEN = 'device-settings';
  var UNLOCK = 'users-unlock';
  var TICK = 'ds-tick';
  var TICK_MS = 1000;
  var LOCKED_HINT = 'Unlock first';

  function build(deps) {
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;
    var now = typeof deps.now === 'function' ? deps.now : function () { return Date.now(); };

    function tick() { return [{ cancelTimer: TICK }, { startTimer: { name: TICK, ms: TICK_MS } }]; }

    var defs = {};
    defs[SCREEN] = {
      title: 'Device settings',
      hint: { text: 'knob: move \\u00b7 knob push: select \\u00b7 esc (hold): back', pinned: true },
      onEnter: function () { return tick(); },
      onLeave: function () { return [{ cancelTimer: TICK }]; },
      rows: function (ctx) {
        var unlocked = Sel.isUnlocked(ctx.domain);
        var first = unlocked
          ? {
            id: 'ds-unlock', label: DL.unlockLabel(true, Sel.deviceUnlockRemainingSecs(ctx.domain, now())),
            kind: 'action', afterSelect: 'none', activate: [{ send: Cmd.userLockSession() }],
          }
          : { id: 'ds-unlock', label: DL.unlockLabel(false), kind: 'action', afterSelect: 'none', activate: [{ push: UNLOCK }] };
        function gated(row) {
          row.disabled = !unlocked;
          if (!unlocked) row.disabledHint = LOCKED_HINT;
          return row;
        }
        return [
          first,
          gated({ id: 'ds-users', label: 'Users', kind: 'nav', target: 'users-list' }),
          gated({ id: 'ds-groups', label: 'Groups', kind: 'nav', target: 'users-groups' }),
          gated({ id: 'ds-reboot', label: 'Reboot device', kind: 'action', afterSelect: 'none', hold: true, activate: [{ send: Cmd.deviceReboot() }] }),
          gated({ id: 'ds-usb', label: 'USB mode', kind: 'nav', target: 'usb-mode' }),
        ];
      },
    };

    var reactions = {
      device_rebooting: function () { return [{ toast: 'Rebooting...' }]; },
    };

    var timers = {};
    timers[TICK] = function (ctx) { return ctx.stack.indexOf(SCREEN) === -1 ? [] : tick(); };

    return { defs: defs, reactions: reactions, timers: timers, initialUi: function () { return {}; } };
  }

  var api = { build: build, SCREEN: SCREEN, TICK: TICK };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensDeviceSettings = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
