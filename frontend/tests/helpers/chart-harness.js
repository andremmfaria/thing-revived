// Builds the real chart + screens + reducers for pure tests, and a small
// driver that mimics boot's event loop without a DOM.
'use strict';
const Config = require('../../src/utils/config.js');
const Reducers = require('../../src/state/reducers.js');
const Sel = require('../../src/state/selectors.js');
const WL = require('../../src/wifi/wifi-logic.js');
const Bridge = require('../../src/transport/bridge-client.js');
const Screens = require('../../src/menu/screens/index.js');
const Chart = require('../../src/interaction/menu-chart.js');
const MenuView = require('../../src/menu/menu-view.js');
const Events = require('../../src/interaction/events.js');

function makeHarness(userCfg) {
  const cfg = Config.create(userCfg || {}, () => {}).values;
  const screens = Screens.build({ WL, Cmd: Bridge.commands, Sel, cfg });
  const chart = Chart.create({ screens, cfg });
  let domain = Reducers.initial(cfg);
  const h = {
    cfg, chart, screens, Events,
    state: chart.initial(domain),
    effects: [], // every effect emitted so far, in order
    get domain() { return domain; },
    // Feeds one event and records its effects; returns the effects of that event.
    send(ev) {
      const r = chart.transition(h.state, ev);
      h.state = r.state;
      h.effects.push(...r.effects);
      return r.effects;
    },
    // A controller message: reduce first, then tell the chart (like boot does).
    wire(msg) {
      domain = Reducers.reduce(domain, msg);
      return h.send(Events.wire(msg, domain));
    },
    // Applies the domain-changing effects the way boot's effect runner would.
    settleDomain(effects) {
      effects.forEach((e) => {
        if (e.dispatch) { domain = Reducers.reduce(domain, e.dispatch); h.send(Events.domain(domain)); }
      });
    },
    knob(d) { return h.send(Events.knob(d)); },
    btn(code, value) { return h.send(Events.button(code, value)); },
    press(code) { h.btn(code, 1); h.btn(code, 0); },
    push() { const fx = h.btn(28, 1); h.settleDomain(fx); return fx; },
    back() { h.btn(2, 1); },
    escDown() { return h.btn(1, 1); },
    escUp() { return h.btn(1, 0); },
    timer(name) { return h.send(Events.timerFired(name)); },
    view() { return MenuView.viewModel(h.state, chart); },
    labels() { return h.view().rows.map((r) => r.label); },
    sent() { return h.effects.filter((e) => e.send).map((e) => e.send); },
    sentSince(n) { return h.effects.slice(n).filter((e) => e.send).map((e) => e.send); },
    mark() { return h.effects.length; },
    open() { h.btn(1, 1); h.btn(1, 0); },
    // Moves the highlight to the row whose label starts with `label`.
    moveTo(label) {
      for (let i = 0; i < 50; i++) {
        const v = h.view();
        const idx = v.rows.findIndex((r) => r.label.indexOf(label) === 0);
        if (idx < 0) throw new Error('no row ' + label + ' in ' + JSON.stringify(v.rows.map((r) => r.label)));
        if (idx === v.highlight) return;
        h.knob(idx > v.highlight ? 1 : -1);
      }
      throw new Error('moveTo stuck');
    },
    choose(label) { h.moveTo(label); return h.push(); },
  };
  return h;
}

module.exports = { makeHarness };
