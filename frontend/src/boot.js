/*
 * Composition root: the only file that reaches for window globals. It builds
 * the channels, store, interaction chart, effects interpreter and views,
 * wires them together and starts the transport. Everything else receives its
 * collaborators as arguments.
 *
 * Data flow:
 *   bridge channel -> bridge client -> knob/button events -> chart
 *                                   -> wire message -> store reducer, then chart
 *   agent channel  -> agent client  -> sessions controller / store / clock
 *   DOM (keyboard, text field, row taps) -> events -> chart
 *   chart -> {state, effects}; state -> menu view, effects -> interpreter
 *   store -> tabs, status, session label, and a `domain` event to the chart
 *
 * Plain ES2018 subset, classic script. In the browser it runs start(window) at
 * load; under node it only exports start for tests.
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var NEEDED = [
    'config', 'logging', 'types', 'module', 'bus', 'timers', 'channel', 'agentClient', 'bridgeClient', 'store', 'reducers', 'selectors',
    'wifiLogic', 'usersLogic', 'wifiPickerView', 'usersSecretView', 'buttons', 'events', 'machine', 'menuChart', 'effects', 'screens', 'menuView',
    'sessions', 'terminalView', 'tabsView', 'keyboard', 'focus', 'clock',
  ];

  var IDS = {
    overlay: 'menu-overlay', title: 'menu-title', list: 'menu-list', toast: 'menu-toast', hint: 'menu-hint',
    inputWrap: 'wifi-input-wrap',
  };
  var TEXT_FIELD_ID = 'wifi-pass-input';

  function start(env) {
    var TR = env.TR;
    TR.module.assertLoaded(NEEDED, TR);
    var Types = TR.types;
    var doc = env.document;
    var cfg = TR.config.values;
    var log = TR.logging.child('boot');

    var timers = TR.timers.create(env);
    var store = TR.store.create(TR.reducers.reduce, TR.reducers.initial(cfg));

    // ---- interaction chart + event loop -----------------------------------------
    var screens = TR.screens.build({ WL: TR.wifiLogic, UL: TR.usersLogic, Cmd: TR.bridgeClient.commands, Sel: TR.selectors, cfg: cfg });
    var chart = TR.menuChart.create({ screens: screens, cfg: cfg });
    var ix = chart.initial(store.getState());
    var queue = [];
    var draining = false;
    var renderMenu = null; // set once the DOM views exist
    var effects = null;

    // Events can arrive while one is being processed (an effect dispatches to
    // the store, a blur follows a focus change); they queue, never nest.
    function dispatch(event) {
      queue.push(event);
      if (draining) return;
      draining = true;
      try {
        while (queue.length) {
          var r = chart.transition(ix, queue.shift());
          var changed = r.state !== ix;
          ix = r.state;
          if (changed && renderMenu) renderMenu();
          if (effects) effects.run(r.effects);
        }
      } catch (err) {
        log.error('interaction failed:', err);
        queue = [];
      } finally {
        draining = false;
      }
    }

    // ---- terminal ------------------------------------------------------------------
    var agentChannel = null;
    var agent = null;
    var bridge = null;

    var termView = TR.terminalView.create({
      Terminal: env.Terminal,
      FitAddon: env.FitAddon.FitAddon,
      ui: cfg.ui,
      terminal: cfg.terminal,
      onResize: function (cols, rows) {
        if (agent) agent.sendResize(store.getState().sessions.active, cols, rows);
      },
    });
    var replay = TR.sessions.createReplayBuffers(cfg.terminal.replay_buffer_bytes);
    var sessions = TR.sessions.createSessions({
      store: store, replay: replay, termView: termView,
      agent: { sendControl: function (i, a) { return agent.sendControl(i, a); } },
    });

    // ---- views -----------------------------------------------------------------------
    var tabs = TR.tabsView.create(doc);
    var focus = TR.focus.create({ doc: doc, termFocus: function () { termView.focus(); } });
    var secretViews = TR.usersSecretView.create(doc);
    var menuRender = TR.menuView.createRenderer(doc, IDS, {
      'wifi-picker': TR.wifiPickerView.create(doc),
      'users-bootstrap': secretViews.bootstrap,
      'users-ack': secretViews.ack,
    }, dispatch);
    renderMenu = function () { menuRender(TR.menuView.viewModel(ix, chart)); };
    var clock = TR.clock.create({
      doc: doc, timers: timers,
      getClock: function () { return store.getState().clock; },
      format: cfg.clock.format, intervalMs: cfg.clock.interval_ms,
    });

    // Tabs and status are only rebuilt when their slice of the state changed.
    var lastSessions = null;
    var lastConnection = null;
    var lastClock = null;
    function renderChrome(domain) {
      if (domain.sessions !== lastSessions) { lastSessions = domain.sessions; tabs.render(domain); }
      if (domain.connection !== lastConnection) { lastConnection = domain.connection; tabs.renderConnection(domain); }
      // A clock_sync redraws immediately; the interval keeps the seconds moving.
      if (domain.clock !== lastClock) { if (lastClock !== null) clock.render(); lastClock = domain.clock; }
    }
    store.subscribe(function (domain) {
      renderChrome(domain);
      dispatch(TR.events.domain(domain));
    });

    // ---- effects -------------------------------------------------------------------------
    var named = TR.timers.createNamedTimers(timers, function (name) { dispatch(TR.events.timerFired(name)); });
    effects = TR.effects.create({
      bridge: { send: function (msg) { return bridge.send(msg); } },
      store: store,
      named: named,
      term: termView,
      sessions: sessions,
      text: focus.textAdapter(TEXT_FIELD_ID),
    });

    // ---- transport -----------------------------------------------------------------------
    var backoff = cfg.reconnect;
    agentChannel = TR.channel.create({ name: 'agent', url: cfg.agent.url, WebSocketImpl: env.WebSocket, timers: timers, backoff: backoff, queueMax: cfg.transport.queue_max });
    var bridgeChannel = TR.channel.create({ name: 'bridge', url: cfg.bridge.url, WebSocketImpl: env.WebSocket, timers: timers, backoff: backoff, queueMax: cfg.transport.queue_max });

    // Decoded frames travel over the bus; a throwing subscriber is logged and
    // does not stop the others.
    var bus = TR.bus.create({ onError: function (topic, err) { log.error('handler for', topic, 'failed:', err); } });
    agent = TR.agentClient.create(agentChannel, function (ev) { bus.emit('agent', ev); });
    bridge = TR.bridgeClient.create(bridgeChannel, function (ev) { bus.emit('bridge', ev); });
    bus.on('agent', function (ev) {
      if (ev.type === 'session_list') sessions.onSessionList(ev.sessions);
      else if (ev.type === 'output') sessions.onOutput(ev.session, ev.data);
      else if (ev.type === 'exited') sessions.onExited(ev);
      else if (ev.type === 'clock_sync') store.dispatch({ type: 'clock_sync', offset_ms: ev.offset_ms, status: ev.status });
    });
    bus.on('bridge', function (ev) {
      if (ev.type === 'knob') dispatch(TR.events.knob(ev.delta));
      else if (ev.type === 'button') dispatch(TR.events.button(ev.code, ev.value));
      else {
        store.dispatch(ev.msg);
        dispatch(TR.events.wire(ev.msg, store.getState()));
      }
    });

    agentChannel.onStatus(function (status) {
      store.dispatch({ type: 'connection', which: 'agent', status: status });
      if (status === Types.ChannelStatus.live) termView.fitAndResize();
    });
    bridgeChannel.onStatus(function (status) {
      store.dispatch({ type: 'connection', which: 'bridge', status: status });
      // The chart wipes a revealed one-time password when the controller link drops.
      if (status !== Types.ChannelStatus.live) dispatch(TR.events.wire({ type: 'connection', which: 'bridge', status: status }, store.getState()));
      // Pull current settings up front so the menu has real values to show.
      if (status === Types.ChannelStatus.live) bridge.requestInitialState();
    });
    agentChannel.onDrop(function (m) { dispatch(TR.events.sendFailed(m.type, 'agent')); });
    bridgeChannel.onDrop(function (m) { dispatch(TR.events.sendFailed(m.type, 'bridge')); });

    // ---- DOM wiring --------------------------------------------------------------------------
    var screenEl = doc.getElementById('screen');
    termView.open(screenEl);
    // Kiosk window is fixed-size in practice, but refit on any resize
    // (orientation/DPI change, devtools) rather than assuming it never changes.
    env.addEventListener('resize', function () { termView.fitAndResize(); });
    TR.keyboard.bind(
      screenEl,
      function () { return ix.mode !== Types.Mode.TERMINAL; },
      function (data) { agent.sendInput(store.getState().sessions.active, data); }
    );
    focus.bindRefocus(doc.querySelector('.term-shell'), TEXT_FIELD_ID);
    focus.bindTextField(TEXT_FIELD_ID, dispatch);
    var closeBtn = doc.getElementById('menu-close');
    if (closeBtn) closeBtn.addEventListener('click', function () { dispatch(TR.events.closeClick()); });
    focus.focusScreen();

    renderChrome(store.getState());
    renderMenu();
    agentChannel.start();
    bridgeChannel.start();
    clock.start();
    log.info('started');
    return { dispatch: dispatch, store: store, chart: chart, getState: function () { return ix; } };
  }

  var api = { start: start, NEEDED: NEEDED };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.boot = api;
    api.app = start(root);
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
