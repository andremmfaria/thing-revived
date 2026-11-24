/*
 * Shared pieces of the account screens (docs/research/user-management-plan.md
 * section 10, task T-P2.6): the text field registry,
 * the locked-session gate and small row builders. Not a screen module itself;
 * accounts.js and accounts-groups.js call create(deps) and build on it.
 *
 * All account knowledge lives in users/users-logic.js and the selectors; this
 * file only turns it into rows. Passwords stay in ui.users.* while their
 * screen is open and are wiped by that screen's onLeave.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensUsersKit).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var HOLD_HINT = 'knob push (hold): confirm \u00b7 esc (hold): back';
  var KEY_MAX = 8192;

  // ui.users key -> how its text field behaves. `secret` fields are masked
  // unless ui.users.showPw is on.
  var FIELDS = {
    admin: { secret: false, max: 31, ph: 'Admin user name' },
    pw: { secret: true, max: 128, ph: 'Admin password' },
    addName: { secret: false, max: 31, ph: 'New user name' },
    addPw: { secret: true, max: 128, ph: 'New password' },
    newPw: { secret: true, max: 128, ph: 'New password' },
    curPw: { secret: true, max: 128, ph: 'Current password' },
    groupName: { secret: false, max: 31, ph: 'New group name' },
    keyLine: { secret: false, max: KEY_MAX, ph: 'Paste one public key line' },
  };

  function create(deps) {
    var UL = deps.UL;
    var WL = deps.WL;
    var Sel = deps.Sel;

    function ui(ctx) { return ctx.ui.users; }
    function unlocked(ctx) { return Sel.isUnlocked(ctx.domain); }
    function patch(key, value) {
      var p = {};
      p[key] = value;
      return { ui: { users: p } };
    }
    function clip(text, max) {
      return text.length > max ? text.slice(0, max - 3) + '...' : text;
    }

    function lockedRows() {
      return [
        { id: 'users-locked', label: 'Admin session is locked', kind: 'label' },
        { id: 'users-relock', label: 'Unlock...', kind: 'action', afterSelect: 'none', activate: [{ popTo: 'device-settings' }, { push: 'users-unlock' }] },
      ];
    }

    // Wraps a screen def: rows are replaced by a locked notice when the admin
    // session is gone (drop, expiry, lock).
    // opts.open: the screen works without a session (the unlock screen).
    function screen(def, opts) {
      var open = !!(opts && opts.open);
      var out = Object.assign({}, def);
      out.rows = function (ctx) {
        if (!open && !unlocked(ctx)) return lockedRows();
        return def.rows(ctx);
      };
      if (def.textField) {
        out.textField = {
          hint: 'type, then enter or esc: done',
          value: function (c) { return ui(c)[ui(c).field]; },
          masked: function (c) { return FIELDS[ui(c).field].secret && !ui(c).showPw; },
          maxLength: function (c) { return FIELDS[ui(c).field].max; },
          placeholder: function (c) { return FIELDS[ui(c).field].ph; },
          onInput: function (c, value) { return [patch(ui(c).field, value)]; },
        };
      }
      return out;
    }

    // A row that opens the shared <input> for ui.users[key].
    function fieldRow(ctx, id, label, key) {
      var v = ui(ctx)[key];
      var shown = !v.length ? '(empty)' : (FIELDS[key].secret ? WL.maskPassword(v, ui(ctx).showPw) : clip(v, 30));
      return {
        id: id, label: label, kind: 'field', afterSelect: 'none', valueText: shown,
        activate: [patch('field', key), { textEdit: 'start' }],
      };
    }

    function toggleRow(ctx, id, label, key) {
      var on = ui(ctx)[key] === true;
      return { id: id, label: label, kind: 'toggle', display: on ? 'ON' : 'OFF', activate: [patch(key, !on)] };
    }

    function cancelRow() {
      return { id: 'users-cancel', label: 'Cancel', kind: 'action', afterSelect: 'pop', activate: [] };
    }

    function note(id, label) { return { id: id, label: label, kind: 'label' }; }

    function names(ctx) {
      var out = [];
      (Sel.userList(ctx.domain) || []).forEach(function (u) { out.push(u.name); });
      (Sel.groupList(ctx.domain) || []).forEach(function (g) { out.push(g.name); });
      return out;
    }

    return {
      HOLD_HINT: HOLD_HINT, FIELDS: FIELDS, KEY_MAX: KEY_MAX,
      ui: ui, unlocked: unlocked, patch: patch, clip: clip, screen: screen,
      fieldRow: fieldRow, toggleRow: toggleRow, cancelRow: cancelRow, note: note, names: names,
      lockedRows: lockedRows,
    };
  }

  var api = { create: create, HOLD_HINT: HOLD_HINT, FIELDS: FIELDS };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensUsersKit = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
