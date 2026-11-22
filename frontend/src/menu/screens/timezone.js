/*
 * Timezone screens: IANA regions first, then the zones of one region.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensTimezone).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function build(deps) {
    var Cmd = deps.Cmd;
    var Sel = deps.Sel;

    var defs = {
      'timezone-groups': {
        title: 'Timezone region',
        onEnter: function (ctx) {
          return ctx.domain.timezone.loaded ? [] : [{ send: Cmd.timezoneListGet() }];
        },
        rows: function (ctx) {
          var list = ctx.domain.timezone.list;
          var groups = Sel.timezoneGroups(list);
          if (groups.length === 0) return [{ id: 'tz-loading', label: 'Loading...', kind: 'label' }];
          return groups.map(function (g) {
            var hasChildren = list.some(function (z) { return z.indexOf(g + '/') === 0; });
            if (hasChildren) {
              return {
                id: 'tz-group-' + g, label: g, kind: 'nav-dynamic', target: 'timezone-zones',
                enter: [{ ui: { tz: { group: g } } }],
              };
            }
            return {
              id: 'tz-zone-' + g, label: g, kind: 'action', afterSelect: 'root',
              activate: [{ send: Cmd.timezoneSet(g) }],
            };
          });
        },
      },
      'timezone-zones': {
        title: function (ctx) { return ctx.ui.tz.group || 'Timezone'; },
        rows: function (ctx) {
          return Sel.zonesInGroup(ctx.domain.timezone.list, ctx.ui.tz.group).map(function (z) {
            return {
              id: 'tz-' + z, label: z, kind: 'action', afterSelect: 'root',
              activate: [{ send: Cmd.timezoneSet(z) }],
            };
          });
        },
      },
    };

    return { defs: defs, reactions: {}, initialUi: function () { return { tz: { group: null } }; } };
  }

  var api = { build: build };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensTimezone = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
