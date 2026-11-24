/*
 * Read-only views over the domain state, shared by the views and the screen
 * definitions. Pure.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.selectors).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var Types = node ? require('../utils/types.js') : root.TR.types;

  function sessionName(domain, index) {
    var list = domain.sessions.list;
    for (var i = 0; i < list.length; i++) {
      if (list[i].index === index) return list[i].name;
    }
    return null;
  }

  function sessionExists(domain, index) {
    return sessionName(domain, index) !== null;
  }

  function sessionLabel(domain) {
    var a = domain.sessions.active;
    return 'session: ' + (sessionName(domain, a) || a);
  }

  function tabs(domain) {
    return domain.sessions.list.map(function (s) {
      return { name: s.name, active: s.index === domain.sessions.active };
    });
  }

  var STATUS_TEXT = { connecting: 'CONNECTING', live: 'LIVE', disconnected: 'DISCONNECTED', error: 'ERROR' };

  // which: 'agent' | 'bridge'
  function connection(domain, which) {
    var status = domain.connection[which];
    return { ok: status === Types.ChannelStatus.live, label: which.toUpperCase() + ': ' + STATUS_TEXT[status] };
  }

  // Groups the real timezone list (about 600 names) into IANA regions plus
  // bare names such as "UTC", so the top screen stays a short fixed list.
  function timezoneGroups(list) {
    var seen = {};
    var groups = [];
    list.forEach(function (z) {
      var slash = z.indexOf('/');
      var g = slash === -1 ? z : z.slice(0, slash);
      if (!seen[g]) {
        seen[g] = true;
        groups.push(g);
      }
    });
    groups.sort();
    return groups;
  }

  function zonesInGroup(list, group) {
    return list.filter(function (z) { return z.indexOf(group + '/') === 0; }).sort();
  }

  // ---- USB role (domain.usb) ----

  var USB_NAMES = { device: 'Device', host: 'Host' };

  // Display name of a wire role; `fallback` (default '...') when unknown.
  function usbRoleName(role, fallback) {
    return Object.prototype.hasOwnProperty.call(USB_NAMES, role) ? USB_NAMES[role] : (fallback || '...');
  }

  // The persisted boot role, or null when none is set or an ADB build ignores it.
  function usbBootRole(usb) { return usb.adbForced ? null : usb.bootRole; }

  // Settings row label: the running role, plus the next-boot role when it differs.
  function usbModeLabel(usb) {
    var label = 'USB mode: ' + usbRoleName(usb.role);
    var next = usbBootRole(usb);
    if (usb.role && next && next !== usb.role) label += ' (' + usbRoleName(next) + ' next boot)';
    return label;
  }

  // ---- debug access (domain.debugAccess): runtime ADB / UART toggles ----

  // True while either surface is enabled: drives the persistent badge.
  function debugAccessActive(domain) {
    var d = domain.debugAccess;
    return d.adb === true || d.uart === true;
  }

  // ADB is impossible while debug_state.blocked is set (today, only
  // 'usb_role_host': the live USB role is host). Not a derived guess: the
  // controller is the one deciding and naming the reason.
  function debugAccessAdbBlocked(domain) { return domain.debugAccess.blocked !== null; }

  function debugAccessError(domain) { return domain.debugAccess.error; }

  // ---- users (Phase 2a/2b), all read ui.users = domain.users ----

  function isUnlocked(domain) { return domain.users.session.unlocked === true; }

  // Name of the admin the session was opened for, or null.
  function sessionUser(domain) { return domain.users.session.username; }

  // user_list.users or null before the first user_list.
  function userList(domain) { return domain.users.list; }

  function userByName(domain, name) {
    var list = domain.users.list || [];
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i];
    return null;
  }

  function adminCount(domain) {
    return (domain.users.list || []).filter(function (u) { return u.admin && !u.locked; }).length;
  }

  function groupList(domain) { return domain.users.groups; }

  // Groups an operator may toggle for a user: only assignable ones, each with
  // whether `name` is a member now (from the user's own group list).
  function membership(domain, name) {
    var u = userByName(domain, name);
    var have = u ? u.groups : [];
    return (domain.users.groups || []).filter(function (g) { return g.assignable; }).map(function (g) {
      return { name: g.name, system: g.system, member: have.indexOf(g.name) !== -1 };
    });
  }

  // The key list for `name`, or null when none (or another user's) was loaded.
  function keysFor(domain, name) {
    var k = domain.users.keys;
    return k && k.name === name ? k.list : null;
  }

  function lastResult(domain) { return domain.users.lastResult; }
  function lastError(domain) { return domain.users.lastError; }

  // {name, password} of a generated password awaiting display, or null.
  function pendingSecret(domain) { return domain.users.secret; }

  var api = {
    usbRoleName: usbRoleName,
    usbBootRole: usbBootRole,
    usbModeLabel: usbModeLabel,
    debugAccessActive: debugAccessActive,
    debugAccessAdbBlocked: debugAccessAdbBlocked,
    debugAccessError: debugAccessError,
    isUnlocked: isUnlocked,
    sessionUser: sessionUser,
    userList: userList,
    userByName: userByName,
    adminCount: adminCount,
    groupList: groupList,
    membership: membership,
    keysFor: keysFor,
    lastResult: lastResult,
    lastError: lastError,
    pendingSecret: pendingSecret,
    sessionName: sessionName,
    sessionExists: sessionExists,
    sessionLabel: sessionLabel,
    tabs: tabs,
    connection: connection,
    timezoneGroups: timezoneGroups,
    zonesInGroup: zonesInGroup,
  };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.selectors = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
