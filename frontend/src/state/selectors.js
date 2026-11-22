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

  var api = {
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
