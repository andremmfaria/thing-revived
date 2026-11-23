/*
 * Account menu screens, core (docs/research/user-management-plan.md section
 * 10, task T-P2.6): unlock with the admin password, user list, user detail
 * (lock, password, remove), add user, one-time password display, the
 * destructive confirms, and every controller reaction of the whole section
 * (the group and key screens are in accounts-groups.js). Replaces the old
 * first-boot reveal screens; first boot is firstboot/first-boot.js now.
 *
 * Rules: entering Users while locked goes to users-unlock; leaving the
 * section, a lost session or 5 idle minutes sends user_lock_session or
 * returns to the unlock screen; the generated password (user_secret) is
 * shown once on users-secret and cleared on leaving it and on lock. Typed
 * passwords live in ui.users.* only while their screen is open. Destructive
 * rows carry `hold: true` (knob push held, see interaction/menu-chart.js).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensAccounts).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Kit = node ? require('./users-kit.js') : TR.screensUsersKit;

  var UNLOCK = 'users-unlock';
  var LIST = 'users-list';
  var USER = 'users-user';
  var SECRET = 'users-secret';
  var SECTION = [LIST, USER, 'users-add', 'users-pw', SECRET, 'users-lock-confirm', 'users-remove-confirm',
    'users-user-groups', 'users-group-confirm', 'users-groups', 'users-group-actions', 'users-group-add',
    'users-group-remove-confirm', 'users-keys', 'users-key-actions', 'users-key-add', 'users-key-remove-confirm'];

  function build(deps) {
    var UL = deps.UL;
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;
    var k = Kit.create(deps);
    var ui = k.ui;

    function inSection(ctx) {
      return SECTION.some(function (id) { return ctx.stack.indexOf(id) !== -1; });
    }

    function clearForms() {
      return [{
        ui: { users: {
          pw: '', curPw: '', newPw: '', addPw: '', addName: '', keyLine: '', groupName: '', showPw: false, field: 'admin',
          addAdmin: false, addGen: true, newGen: true, keepHome: false, back: null,
        } },
      }, { text: 'reset' }];
    }

    // The screen to return to after the one-time password: the user's page
    // when it is on the stack, the list otherwise.
    function secretTarget(ctx) { return ctx.stack.indexOf(USER) !== -1 ? USER : LIST; }

    function tags(u) {
      var t = [];
      if (u.admin) t.push('admin');
      if (u.locked) t.push('locked');
      if (u.protected) t.push('protected');
      return t.join(' ');
    }

    var defs = {};

    // ---- unlock ----------------------------------------------------------------
    function submitUnlock(ctx) {
      var u = ui(ctx);
      var admin = u.admin.trim();
      if (!admin) return [{ toast: 'Enter the admin user name' }];
      if (!u.pw) return [{ toast: 'Enter the admin password' }];
      // The password leaves ui.users the moment it is sent.
      return [{ ui: { users: { pw: '', showPw: false } } }, { text: 'reset' }, { send: Cmd.userUnlock(admin, u.pw) }];
    }

    defs[UNLOCK] = k.screen({
      title: 'Unlock accounts',
      hint: 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back',
      textField: true,
      onEnter: function () { return [{ ui: { users: { pw: '', showPw: false, field: 'admin' } } }]; },
      onLeave: function () { return [{ ui: { users: { pw: '', showPw: false } } }, { text: 'reset' }]; },
      rows: function (ctx) {
        return [
          k.fieldRow(ctx, 'users-unlock-admin', 'Admin', 'admin'),
          k.fieldRow(ctx, 'users-unlock-pw', 'Password', 'pw'),
          k.toggleRow(ctx, 'users-unlock-show', 'Show password', 'showPw'),
          { id: 'users-unlock-go', label: 'Unlock', kind: 'action', afterSelect: 'none', activate: submitUnlock(ctx) },
        ];
      },
    }, { open: true });

    // ---- list ------------------------------------------------------------------
    defs[LIST] = k.screen({
      title: 'Users',
      onEnter: function () { return [{ send: Cmd.userListGet() }, { send: Cmd.groupListGet() }]; },
      // Leaving the section ends the admin session and forgets what was shown.
      onLeave: function (ctx) {
        var cmds = [{ cancelTimer: k.IDLE }, { dispatch: { type: 'local/users_leave' } }].concat(clearForms());
        if (k.unlocked(ctx)) cmds.push({ send: Cmd.userLockSession() });
        return cmds;
      },
      rows: function (ctx) {
        var list = Sel.userList(ctx.domain);
        var rows = [];
        if (list === null) {
          rows.push(k.note('users-loading', 'Loading...'));
        } else {
          var shown = list.filter(function (u) { return !u.system; });
          if (shown.length === 0) rows.push(k.note('users-none', 'No users'));
          shown.forEach(function (u) {
            rows.push({
              id: 'users-u-' + u.name, label: u.name, kind: 'nav-dynamic', target: USER, valueText: tags(u),
              enter: [{ ui: { users: { sel: u.name } } }],
            });
          });
        }
        rows.push({ id: 'users-add', label: 'Add user...', kind: 'nav', target: 'users-add' });
        rows.push({ id: 'users-groups', label: 'Groups...', kind: 'nav', target: 'users-groups' });
        rows.push({ id: 'users-lock-now', label: 'Lock admin session', kind: 'action', afterSelect: 'root', activate: [] });
        return rows;
      },
    });

    // ---- one user --------------------------------------------------------------
    defs[USER] = k.screen({
      title: function (ctx) { return ui(ctx).sel || 'User'; },
      onEnter: function () { return [{ send: Cmd.userListGet() }]; },
      rows: function (ctx) {
        var name = ui(ctx).sel;
        var user = Sel.userByName(ctx.domain, name);
        if (!user) {
          return [k.note('users-gone', 'User not found'), { id: 'users-gone-back', label: 'Back', kind: 'action', afterSelect: 'pop', activate: [] }];
        }
        var rows = [
          k.note('users-st', 'Account: ' + (user.locked ? 'locked' : 'active') + (user.admin ? ', admin' : '')),
          k.note('users-gr', 'Groups: ' + (user.groups.length ? user.groups.join(', ') : 'none')),
        ];
        var lockBlock = UL.userBlock(user, user.locked ? 'unlock' : 'lock');
        if (lockBlock) {
          rows.push(k.note('users-lock-blocked', 'Lock: ' + lockBlock));
        } else if (user.locked) {
          rows.push({
            id: 'users-unlock-acct', label: 'Unlock account', kind: 'action', afterSelect: 'none',
            activate: [k.patch('back', null), { send: Cmd.userSetLocked(name, false, '') }],
          });
        } else {
          rows.push({ id: 'users-lock-acct', label: 'Lock account...', kind: 'nav', target: 'users-lock-confirm' });
        }
        var pwBlock = UL.userBlock(user, 'password');
        if (pwBlock) rows.push(k.note('users-pw-blocked', 'Password: ' + pwBlock));
        else rows.push({ id: 'users-pw', label: 'Set password...', kind: 'nav', target: 'users-pw' });
        rows.push({ id: 'users-ug', label: 'Groups...', kind: 'nav', target: 'users-user-groups' });
        rows.push({
          id: 'users-keys', label: 'SSH keys...', kind: 'nav', target: 'users-keys',
          valueText: typeof user.key_count === 'number' ? String(user.key_count) : '',
        });
        var rmBlock = UL.userBlock(user, 'remove');
        if (rmBlock) rows.push(k.note('users-rm-blocked', 'Remove: ' + rmBlock));
        else rows.push({ id: 'users-rm', label: 'Remove user...', kind: 'nav', target: 'users-remove-confirm' });
        return rows;
      },
    });

    // ---- add -------------------------------------------------------------------
    function submitAdd(ctx) {
      var u = ui(ctx);
      var name = u.addName;
      var err = UL.validateName(name, k.names(ctx));
      if (!err && !u.addGen) err = UL.validatePassword(name, u.addPw);
      if (err) return [{ toast: err }];
      return [k.patch('back', LIST), { send: Cmd.userAdd(name, u.addGen ? '' : u.addPw, u.addAdmin === true) }];
    }

    defs['users-add'] = k.screen({
      title: 'Add user',
      hint: 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back (discards the form)',
      textField: true,
      onEnter: function () { return clearForms(); },
      onLeave: function () { return clearForms(); },
      rows: function (ctx) {
        var u = ui(ctx);
        var rows = [
          k.fieldRow(ctx, 'users-add-name', 'Name', 'addName'),
          k.toggleRow(ctx, 'users-add-admin', 'Administrator', 'addAdmin'),
          k.toggleRow(ctx, 'users-add-gen', 'Generate password', 'addGen'),
        ];
        if (!u.addGen) {
          rows.push(k.fieldRow(ctx, 'users-add-pw', 'Password', 'addPw'));
          rows.push(k.toggleRow(ctx, 'users-add-show', 'Show password', 'showPw'));
        }
        rows.push({ id: 'users-add-go', label: 'Create user', kind: 'action', afterSelect: 'none', activate: submitAdd(ctx) });
        return rows;
      },
    });

    // ---- set password ----------------------------------------------------------
    function submitPw(ctx) {
      var u = ui(ctx);
      var name = u.sel;
      var own = name === Sel.sessionUser(ctx.domain);
      if (!u.newGen) {
        var err = UL.validatePassword(name, u.newPw);
        if (err) return [{ toast: err }];
      }
      if (own && !u.curPw) return [{ toast: 'Enter your current password' }];
      return [
        k.patch('back', USER),
        { send: Cmd.userSetPassword(name, u.newGen ? '' : u.newPw, own ? u.curPw : '', UL.confirmFor(name)) },
      ];
    }

    defs['users-pw'] = k.screen({
      title: function (ctx) { return 'Password: ' + (ui(ctx).sel || ''); },
      hint: 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back (discards the form)',
      textField: true,
      onEnter: function () { return clearForms(); },
      onLeave: function () { return clearForms(); },
      rows: function (ctx) {
        var u = ui(ctx);
        var rows = [];
        if (u.sel === UL.KIOSK_USER) rows.push(k.note('users-pw-ssh', 'This is also the SSH login password'));
        rows.push(k.toggleRow(ctx, 'users-pw-gen', 'Generate new password', 'newGen'));
        if (!u.newGen) {
          rows.push(k.fieldRow(ctx, 'users-pw-new', 'New password', 'newPw'));
        }
        if (u.sel === Sel.sessionUser(ctx.domain)) rows.push(k.fieldRow(ctx, 'users-pw-cur', 'Current password', 'curPw'));
        if (!u.newGen || u.sel === Sel.sessionUser(ctx.domain)) rows.push(k.toggleRow(ctx, 'users-pw-show', 'Show password', 'showPw'));
        rows.push({ id: 'users-pw-go', label: 'Set password', kind: 'action', afterSelect: 'none', activate: submitPw(ctx) });
        return rows;
      },
    });

    // ---- the one-time password -------------------------------------------------
    defs[SECRET] = k.screen({
      title: function (ctx) { var s = Sel.pendingSecret(ctx.domain); return s ? 'Password for ' + s.name : 'Password'; },
      hint: { text: 'shown once \u00b7 write it down \u00b7 esc: done', pinned: true },
      onLeave: function () { return [{ dispatch: { type: 'local/users_clear_secret' } }]; },
      rows: function (ctx) {
        var s = Sel.pendingSecret(ctx.domain);
        var done = { id: 'users-secret-done', label: 'Done', kind: 'action', afterSelect: 'none', activate: [{ popTo: secretTarget(ctx) }] };
        if (!s) return [k.note('users-secret-none', 'Nothing to show'), done];
        return [
          k.note('users-secret-note', 'Shown once. It cannot be shown again.'),
          { id: 'users-secret-pw', label: UL.groups(s.password).join(' '), kind: 'label', cls: 'secret-row' },
          done,
        ];
      },
    });

    // ---- confirms --------------------------------------------------------------
    function atCancel(id) {
      var h = {};
      h[id] = 0;
      return [{ highlight: h }];
    }

    defs['users-lock-confirm'] = k.screen({
      title: function (ctx) { return 'Lock ' + (ui(ctx).sel || '') + '?'; },
      hint: { text: 'ends their sessions \u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return atCancel('users-lock-confirm'); },
      rows: function (ctx) {
        var name = ui(ctx).sel;
        return [
          k.cancelRow(),
          {
            id: 'users-lock-yes', label: 'Lock ' + name, kind: 'action', afterSelect: 'none', hold: true,
            activate: [k.patch('back', USER), { send: Cmd.userSetLocked(name, true, UL.confirmFor(name)) }],
          },
        ];
      },
    });

    defs['users-remove-confirm'] = k.screen({
      title: function (ctx) { return 'Remove ' + (ui(ctx).sel || '') + '?'; },
      hint: { text: 'deletes the account and its SSH keys \u00b7 ' + k.HOLD_HINT, pinned: true },
      onEnter: function () { return atCancel('users-remove-confirm').concat([k.patch('keepHome', false)]); },
      rows: function (ctx) {
        var name = ui(ctx).sel;
        return [
          k.cancelRow(),
          k.toggleRow(ctx, 'users-rm-keep', 'Keep home directory', 'keepHome'),
          {
            id: 'users-rm-yes', label: 'Remove ' + name, kind: 'action', afterSelect: 'none', hold: true,
            activate: [k.patch('back', LIST), { send: Cmd.userRemove(name, ui(ctx).keepHome === true, UL.confirmFor(name)) }],
          },
        ];
      },
    });

    // ---- reactions -------------------------------------------------------------
    var reactions = {
      user_session: function (ctx, msg) {
        if (msg.unlocked === true) {
          if (ctx.top !== UNLOCK) return [];
          return [{ ui: { users: { pw: '', showPw: false } } }, { text: 'reset' }, { pop: true }, { push: LIST }];
        }
        // Locked from the controller side (idle, expiry) while a section
        // screen is up: back to the unlock gate. Our own lock on leaving finds
        // the section already gone.
        if (!inSection(ctx)) return [];
        return [{ cancelTimer: k.IDLE }, { toast: 'Admin session locked' }, { popToRoot: true }, { push: UNLOCK }];
      },
      user_list: function (ctx) {
        var sel = ui(ctx).sel;
        if (ctx.stack.indexOf(USER) !== -1 && sel && Sel.userList(ctx.domain) !== null && !Sel.userByName(ctx.domain, sel)) {
          return [{ popTo: LIST }];
        }
        return [];
      },
      group_list: function (ctx) {
        var g = ui(ctx).selGroup;
        var list = Sel.groupList(ctx.domain);
        var gone = g && list !== null && !list.some(function (x) { return x.name === g; });
        return gone && ctx.stack.indexOf('users-group-actions') !== -1 ? [{ popTo: 'users-groups' }] : [];
      },
      ssh_key_list: function (ctx) {
        var fp = ui(ctx).selKey;
        var list = Sel.keysFor(ctx.domain, ui(ctx).sel);
        var gone = fp && list !== null && !list.some(function (x) { return x.fingerprint === fp; });
        return gone && ctx.stack.indexOf('users-key-actions') !== -1 ? [{ popTo: 'users-keys' }] : [];
      },
      user_secret: function (ctx) {
        if (!inSection(ctx) || !k.unlocked(ctx)) return [{ dispatch: { type: 'local/users_clear_secret' } }];
        if (ctx.top === SECRET) return [];
        return [{ push: SECRET }, { highlight: { 'users-secret': 2 } }];
      },
      user_result: function (ctx, msg) {
        var cmds = [{ toast: UL.resultText(msg) }];
        if (!msg.success || !k.unlocked(ctx)) return cmds;
        // The list the user is looking at may have changed: ask again.
        cmds.push({ send: Cmd.userListGet() });
        if (ctx.stack.indexOf('users-groups') !== -1 || ctx.stack.indexOf('users-user-groups') !== -1) cmds.push({ send: Cmd.groupListGet() });
        if (ctx.stack.indexOf('users-keys') !== -1 && ui(ctx).sel) cmds.push({ send: Cmd.sshKeyListGet(ui(ctx).sel) });
        var back = ui(ctx).back;
        // A generated password already on screen keeps the user on it; its
        // Done button returns.
        if (back && ctx.top !== SECRET && ctx.stack.indexOf(back) !== -1) cmds.push({ popTo: back });
        return cmds;
      },
      user_error: function (ctx, msg) {
        var text = UL.errorText(msg.code, msg.message);
        if (msg.code === 'not_unlocked') {
          if (!inSection(ctx)) return [{ toast: text }];
          return [{ cancelTimer: k.IDLE }, { toast: text }, { popToRoot: true }, { push: UNLOCK }];
        }
        var cmds = [{ toast: text }];
        if (msg.code === 'bad_password' && ctx.top === UNLOCK) cmds.push({ ui: { users: { pw: '' } } });
        return cmds;
      },
    };

    var timers = {};
    timers[k.IDLE] = function (ctx) {
      if (!k.unlocked(ctx)) return [];
      return [{ send: Cmd.userLockSession() }, { toast: 'Admin session locked (idle)' }];
    };

    return {
      defs: defs,
      reactions: reactions,
      timers: timers,
      initialUi: function () {
        return {
          users: {
            field: 'admin', admin: UL.KIOSK_USER, pw: '', showPw: false, sel: null,
            addName: '', addAdmin: false, addGen: true, addPw: '', newGen: true, newPw: '', curPw: '',
            keepHome: false, groupName: '', keyLine: '', selGroup: null, selKey: null, back: null,
          },
        };
      },
    };
  }

  var api = { build: build, SECTION: SECTION, UNLOCK: UNLOCK, LIST: LIST, USER: USER, SECRET: SECRET };

  if (node) {
    module.exports = api;
  } else {
    root.TR.screensAccounts = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
