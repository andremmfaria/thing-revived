/*
 * Screen registry: merges the per-feature screen modules into one
 * {defs, reactions, initialUi} the interaction chart consumes. To add a
 * screen, add a module with build(deps) and list it here and in index.html.
 *
 * deps = {WL, UL, Cmd, Sel, cfg}: wifi-logic, users-logic, the bridge command
 * builders, the domain selectors and the config values.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screens).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var MODULES = node
    ? [require('./root.js'), require('./bluetooth.js'), require('./terminal.js'), require('./timezone.js'), require('./wifi.js'), require('./accounts.js'), require('./accounts-groups.js')]
    : [TR.screensRoot, TR.screensBluetooth, TR.screensTerminal, TR.screensTimezone, TR.screensWifi, TR.screensAccounts, TR.screensAccountsGroups];

  function build(deps) {
    var defs = {};
    var reactions = {};
    var timers = {};
    var uis = [];
    MODULES.forEach(function (m) {
      var part = m.build(deps);
      Object.keys(part.defs).forEach(function (id) {
        if (defs[id]) throw new Error('duplicate screen id ' + id);
        defs[id] = part.defs[id];
      });
      Object.keys(part.reactions).forEach(function (type) {
        if (reactions[type]) throw new Error('duplicate reaction for ' + type);
        reactions[type] = part.reactions[type];
      });
      Object.keys(part.timers || {}).forEach(function (name) {
        if (timers[name]) throw new Error('duplicate timer handler for ' + name);
        timers[name] = part.timers[name];
      });
      uis.push(part.initialUi);
    });
    return {
      defs: defs,
      reactions: reactions,
      timers: timers,
      initialUi: function () {
        var ui = {};
        uis.forEach(function (fn) { Object.assign(ui, fn()); });
        return ui;
      },
    };
  }

  var api = { build: build };

  if (node) module.exports = api;
  else TR.screens = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
