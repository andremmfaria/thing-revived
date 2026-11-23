/*
 * Account menu screens, Phase 2b (docs/research/user-management-plan.md
 * section 10, task T-P2.6): per-user group membership toggles, the group
 * list with add and remove, and the per-user SSH key list with add and
 * remove. Reactions, timers and the shared helpers are in accounts.js and
 * users-kit.js; this module only declares screens.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensAccountsGroups).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Kit = node ? require('./users-kit.js') : TR.screensUsersKit;

  function build(deps) {
    var UL = deps.UL;
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;
    var k = Kit.create(deps);
    var ui = k.ui;

    function atCancel(id) {
      var h = {};
      h[id] = 0;
      return [{ highlight: h }];
    }

    function loading(id) { return [k.note(id, 'Loading...')]; }

    var defs = {};

    // ---- membership of one user ----------------------------------------------
    defs['users-user-groups'] = k.screen({
      title: function (ctx) { return 'Groups: ' + (ui(ctx).sel || ''); },
      onEnter: function () { return [{ send: Cmd.groupListGet() }, { send: Cmd.userListGet() }]; },
      rows: function (ctx) {
        var name = ui(ctx).sel;
        if (Sel.groupList(ctx.domain) === null || Sel.userList(ctx.domain) === null) return loading('users-ug-loading');
        var rows = Sel.membership(ctx.domain, name).map(function (g) {
          var activate;
          if (g.member) {
            activate = [k.patch('back', null), { send: Cmd.userGroupRemove(name, g.name) }];
          } else if (UL.groupNeedsConfirm(g)) {
            activate = [k.patch('selGroup', g.name), { push: 'users-group-confirm' }];
          } else {
            activate = [k.patch('back', null), { send: Cmd.userGroupAdd(name, g.name, '') }];
          }
          return { id: 'users-ug-' + g.name, label: g.name, kind: 'toggle', display: g.member ? 'ON' : 'OFF', activate: activate };
        });
        if (rows.length === 0) rows.push(k.note('users-ug-none', 'No assignable groups'));
        return rows;
      },
    });

    defs['users-group-confirm'] = k.screen({
      title: function (ctx) { return 'Add to ' + (ui(ctx).selGroup || '') + '?'; },
      hint: { text: 'grants extra privileges \u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return atCancel('users-group-confirm'); },
      rows: function (ctx) {
        var name = ui(ctx).sel;
        var group = ui(ctx).selGroup;
        return [
          k.cancelRow(),
          {
            id: 'users-gc-yes', label: 'Add ' + name + ' to ' + group, kind: 'action', afterSelect: 'none', hold: true,
            activate: [k.patch('back', 'users-user-groups'), { send: Cmd.userGroupAdd(name, group, UL.confirmFor(group)) }],
          },
        ];
      },
    });

    // ---- the groups themselves -----------------------------------------------
    defs['users-groups'] = k.screen({
      title: 'Groups',
      onEnter: function () { return [{ send: Cmd.groupListGet() }]; },
      rows: function (ctx) {
        var list = Sel.groupList(ctx.domain);
        if (list === null) return loading('users-g-loading');
        var rows = list.map(function (g) {
          return {
            id: 'users-g-' + g.name, label: g.name, kind: 'nav-dynamic', target: 'users-group-actions',
            valueText: g.system ? 'system' : String(g.members.length) + (g.members.length === 1 ? ' member' : ' members'),
            enter: [k.patch('selGroup', g.name)],
          };
        });
        rows.push({ id: 'users-g-add', label: 'Add group...', kind: 'nav', target: 'users-group-add' });
        return rows;
      },
    });

    defs['users-group-actions'] = k.screen({
      title: function (ctx) { return ui(ctx).selGroup || 'Group'; },
      rows: function (ctx) {
        var name = ui(ctx).selGroup;
        var g = (Sel.groupList(ctx.domain) || []).filter(function (x) { return x.name === name; })[0];
        if (!g) return [k.note('users-ga-gone', 'Group not found'), { id: 'users-ga-back', label: 'Back', kind: 'action', afterSelect: 'pop', activate: [] }];
        var rows = [k.note('users-ga-members', 'Members: ' + (g.members.length ? g.members.join(', ') : 'none'))];
        if (g.system) rows.push(k.note('users-ga-system', 'System group: cannot be removed'));
        else rows.push({ id: 'users-ga-rm', label: 'Remove group...', kind: 'nav', target: 'users-group-remove-confirm' });
        return rows;
      },
    });

    function submitGroup(ctx) {
      var name = ui(ctx).groupName;
      var err = UL.validateName(name, k.names(ctx));
      if (err) return [{ toast: err }];
      return [k.patch('back', 'users-groups'), { send: Cmd.groupAdd(name) }];
    }

    defs['users-group-add'] = k.screen({
      title: 'Add group',
      hint: 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back',
      textField: true,
      onEnter: function () { return [k.patch('groupName', ''), k.patch('field', 'groupName')]; },
      onLeave: function () { return [k.patch('groupName', ''), { text: 'reset' }]; },
      rows: function (ctx) {
        return [
          k.fieldRow(ctx, 'users-ga-name', 'Name', 'groupName'),
          { id: 'users-ga-go', label: 'Create group', kind: 'action', afterSelect: 'none', activate: submitGroup(ctx) },
        ];
      },
    });

    defs['users-group-remove-confirm'] = k.screen({
      title: function (ctx) { return 'Remove ' + (ui(ctx).selGroup || '') + '?'; },
      hint: { text: 'members lose the group \u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return atCancel('users-group-remove-confirm'); },
      rows: function (ctx) {
        var group = ui(ctx).selGroup;
        return [
          k.cancelRow(),
          {
            id: 'users-gr-yes', label: 'Remove group ' + group, kind: 'action', afterSelect: 'none', hold: true,
            activate: [k.patch('back', 'users-groups'), { send: Cmd.groupRemove(group, UL.confirmFor(group)) }],
          },
        ];
      },
    });

    // ---- SSH keys of one user ------------------------------------------------
    defs['users-keys'] = k.screen({
      title: function (ctx) { return 'SSH keys: ' + (ui(ctx).sel || ''); },
      onEnter: function (ctx) { return ui(ctx).sel ? [{ send: Cmd.sshKeyListGet(ui(ctx).sel) }] : []; },
      rows: function (ctx) {
        var list = Sel.keysFor(ctx.domain, ui(ctx).sel);
        var rows = [];
        if (list === null) {
          rows.push(k.note('users-k-loading', 'Loading...'));
        } else {
          if (list.length === 0) rows.push(k.note('users-k-none', 'No keys'));
          list.forEach(function (key) {
            rows.push({
              id: 'users-k-' + key.fingerprint, label: UL.keyLabel(key), kind: 'nav-dynamic', target: 'users-key-actions',
              valueText: UL.shortFingerprint(key.fingerprint, 20),
              enter: [k.patch('selKey', key.fingerprint)],
            });
          });
        }
        rows.push({ id: 'users-k-add', label: 'Add key...', kind: 'nav', target: 'users-key-add' });
        return rows;
      },
    });

    defs['users-key-actions'] = k.screen({
      title: 'SSH key',
      rows: function (ctx) {
        var list = Sel.keysFor(ctx.domain, ui(ctx).sel) || [];
        var fp = ui(ctx).selKey;
        var key = list.filter(function (x) { return x.fingerprint === fp; })[0];
        if (!key) return [k.note('users-ka-gone', 'Key not found'), { id: 'users-ka-back', label: 'Back', kind: 'action', afterSelect: 'pop', activate: [] }];
        return [
          k.note('users-ka-type', UL.keyLabel(key)),
          k.note('users-ka-fp', UL.shortFingerprint(key.fingerprint, 52)),
          { id: 'users-ka-rm', label: 'Remove key...', kind: 'nav', target: 'users-key-remove-confirm' },
        ];
      },
    });

    function submitKey(ctx) {
      var line = ui(ctx).keyLine.trim();
      var err = UL.validateKeyLine(line);
      if (err) return [{ toast: err }];
      return [k.patch('back', 'users-keys'), { send: Cmd.sshKeyAdd(ui(ctx).sel, line) }];
    }

    defs['users-key-add'] = k.screen({
      title: 'Add SSH key',
      hint: 'a keyboard is needed to paste a key \u00b7 esc: edit / select \u00b7 esc (hold): back',
      textField: true,
      onEnter: function () { return [k.patch('keyLine', ''), k.patch('field', 'keyLine')]; },
      onLeave: function () { return [k.patch('keyLine', ''), { text: 'reset' }]; },
      rows: function (ctx) {
        return [
          k.fieldRow(ctx, 'users-ka-line', 'Public key', 'keyLine'),
          { id: 'users-ka-add', label: 'Add key', kind: 'action', afterSelect: 'none', activate: submitKey(ctx) },
        ];
      },
    });

    defs['users-key-remove-confirm'] = k.screen({
      title: 'Remove this key?',
      hint: { text: 'the key stops working at once \u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return atCancel('users-key-remove-confirm'); },
      rows: function (ctx) {
        var fp = ui(ctx).selKey;
        return [
          k.cancelRow(),
          {
            id: 'users-kr-yes', label: 'Remove key', kind: 'action', afterSelect: 'none', hold: true,
            activate: [k.patch('back', 'users-keys'), { send: Cmd.sshKeyRemove(ui(ctx).sel, fp, UL.confirmFor(fp)) }],
          },
        ];
      },
    });

    // Reactions for these screens are in accounts.js (one per message type).
    return { defs: defs, reactions: {}, initialUi: function () { return {}; } };
  }

  var api = { build: build };

  if (node) {
    module.exports = api;
  } else {
    root.TR.screensAccountsGroups = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
