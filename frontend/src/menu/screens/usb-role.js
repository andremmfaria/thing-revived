/*
 * USB mode screen: shows the running USB-C role and the persisted boot role,
 * picks the boot role (device or host) and reboots to apply it. Choosing a
 * role only persists it (usb_role_set); the controller never switches the
 * port live, because host mode drops the USB link to a PC. Reboot now is a
 * hold-to-confirm row, like the destructive Users rows.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensUsbRole).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var SCREEN = 'usb-role';

  function build(deps) {
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;

    function pick(id, label, role, usb) {
      var chosen = Sel.usbBootRole(usb) === role;
      return {
        id: id, label: label, kind: 'action', afterSelect: 'none',
        valueText: chosen ? 'selected' : undefined,
        activate: [{ send: Cmd.usbRoleSet(role) }],
      };
    }

    // Rows are the same ones `onEnter` counts to park the highlight on the
    // first choice, so keep the notes above the choices in one place.
    function notes(usb) {
      var list = [
        { id: 'usb-running', label: 'Running now: ' + Sel.usbRoleName(usb.role), kind: 'label' },
        { id: 'usb-boot', label: 'Next boot: ' + (usb.adbForced ? 'Device (forced)' : Sel.usbRoleName(usb.bootRole, 'Device (default)')), kind: 'label' },
      ];
      if (usb.adbForced) {
        list.push({ id: 'usb-adb', label: 'ADB build: device mode is forced at boot, the saved choice is ignored', kind: 'label' });
      }
      return list;
    }

    var defs = {};
    defs[SCREEN] = {
      title: 'USB mode',
      hint: { text: 'applies from the next boot \u00b7 knob push (hold): reboot \u00b7 esc (hold): back', pinned: true },
      onEnter: function (ctx) {
        return [{ highlight: (function () { var h = {}; h[SCREEN] = notes(ctx.domain.usb).length; return h; })() }, { send: Cmd.usbRoleGet() }];
      },
      rows: function (ctx) {
        var usb = ctx.domain.usb;
        return notes(usb).concat([
          pick('usb-pick-device', 'Boot as Device (USB link to a PC)', 'device', usb),
          pick('usb-pick-host', 'Boot as Host (dock: Wi-Fi, Ethernet, storage)', 'host', usb),
          {
            id: 'usb-reboot', label: 'Reboot now', kind: 'action', afterSelect: 'none', hold: true,
            activate: [{ send: Cmd.usbRoleReboot() }],
          },
          { id: 'usb-warn-1', label: 'Host mode drops the USB link to a PC (ssh, adb) after the next boot.', kind: 'label' },
          { id: 'usb-warn-2', label: 'Recovery: run "superbird-usb-role persist device" on the device terminal, or reflash.', kind: 'label' },
        ]);
      },
    };

    var reactions = {
      usb_role_error: function (ctx, msg) { return [{ toast: 'USB mode: ' + msg.message }]; },
      usb_role_rebooting: function () { return [{ toast: 'Rebooting...' }]; },
    };

    return { defs: defs, reactions: reactions, initialUi: function () { return {}; } };
  }

  var api = { build: build, SCREEN: SCREEN };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensUsbRole = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
