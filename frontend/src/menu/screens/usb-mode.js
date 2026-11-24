/*
 * USB mode screen (docs/research/device-settings-plan.md section 8.1): inline
 * toggle rows Mode (Device/Host), SSH, ADB and UART. The value shown is always
 * the persisted one from the last device_settings event, never optimistic:
 * activating a row only sends device_settings_set with the opposite value.
 * ADB is disabled while the controller reports it blocked (Host mode); every
 * row is disabled while a change is being applied or the session is locked.
 *
 * Layout cap for 800x480: four toggle rows, at most ONE note row (the error
 * takes that slot when there is one) and one pinned hint line.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensUsbMode).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var DL = node ? require('../../device-settings/device-settings-logic.js') : TR.deviceSettingsLogic;

  var SCREEN = 'usb-mode';

  function build(deps) {
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;

    function clearError() { return [{ dispatch: { type: 'local/device_clear_error' } }]; }

    function toggle(ctx, setting) {
      var unlocked = Sel.isUnlocked(ctx.domain);
      var busy = Sel.deviceBusy(ctx.domain) !== null;
      var persisted = ctx.domain.device.persisted[setting];
      var next = DL.nextValue(setting, persisted);
      var row = {
        id: 'um-' + setting, label: DL.settingName(setting), kind: 'toggle', display: DL.valueLabel(setting, persisted),
        activate: next === null ? [] : clearError().concat([{ send: Cmd.deviceSettingsSet(setting, next) }]),
      };
      if (!unlocked) {
        row.disabled = true;
        row.disabledHint = 'Unlock first';
      } else if (setting === 'adb' && Sel.deviceAdbBlocked(ctx.domain)) {
        row.disabled = true;
        row.disabledHint = 'ADB needs Device mode';
      } else if (busy) {
        row.disabled = true;
        row.disabledHint = 'Switching, wait a moment';
      }
      return row;
    }

    var defs = {};
    defs[SCREEN] = {
      title: 'USB mode',
      hint: { text: 'knob push: change. Settings persist across reboots.', pinned: true },
      onEnter: function () { return clearError().concat([{ send: Cmd.deviceSettingsGet() }]); },
      onLeave: function () { return clearError(); },
      rows: function (ctx) {
        var rows = ['mode', 'ssh', 'adb', 'uart'].map(function (s) { return toggle(ctx, s); });
        var err = Sel.deviceError(ctx.domain);
        var text = err ? DL.errorText(err.code, err.message) : (DL.usbNote(ctx.domain.device) || DL.driftNote(ctx.domain.device));
        if (text) rows.push({ id: err ? 'um-error' : 'um-note', label: text, kind: 'label' });
        return rows;
      },
    };

    return { defs: defs, reactions: {}, initialUi: function () { return {}; } };
  }

  var api = { build: build, SCREEN: SCREEN };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensUsbMode = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
