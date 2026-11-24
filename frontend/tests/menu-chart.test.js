'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Machine = require('../src/interaction/machine.js');
const Events = require('../src/interaction/events.js');
const { makeHarness } = require('./helpers/chart-harness.js');

const timer = (name, ms) => ({ startTimer: { name, ms } });
const cancel = (name) => ({ cancelTimer: name });

// ---- machine.js --------------------------------------------------------------------
test('machine: handler lookup walks the dotted path to the root; null falls through', () => {
  const calls = [];
  const m = Machine.create({
    states: {
      '': { ping: () => { calls.push('root'); return { effects: ['r'] }; } },
      a: { ping: () => { calls.push('a'); return null; }, only: (s) => ({ state: Object.assign({}, s, { hit: 'a' }), effects: [] }) },
      'a.b': { ping: () => { calls.push('a.b'); return null; } },
    },
  });
  const r = m.transition({ mode: 'a.b.c' }, { type: 'ping' });
  assert.deepEqual(calls, ['a.b', 'a', 'root']);
  assert.deepEqual(r.effects, ['r']);
  assert.equal(m.transition({ mode: 'a.b' }, { type: 'only' }).state.hit, 'a');
  const unhandled = { mode: 'x' };
  const u = m.transition(unhandled, { type: 'nothing' });
  assert.equal(u.state, unhandled);
  assert.deepEqual(u.effects, []);
  assert.deepEqual(Machine.chain('open.pairing.confirm'), ['open.pairing.confirm', 'open.pairing', 'open', '']);
});

test('machine: transitions never mutate the input state', () => {
  const h = makeHarness();
  const before = JSON.stringify(h.state);
  const frozen = h.state;
  h.open();
  h.knob(1);
  h.btn(28, 1);
  assert.equal(JSON.stringify(frozen), before);
});

// ---- terminal mode -------------------------------------------------------------------
test('terminal: knob scrolls, buttons 1..4 switch sessions, other keys do nothing', () => {
  const h = makeHarness();
  assert.deepEqual(h.knob(3), [{ scroll: 6 }]);
  assert.deepEqual(h.knob(-1), [{ scroll: -2 }]);
  assert.deepEqual(h.btn(3, 1), [{ switchSession: 1 }]);
  assert.deepEqual(h.btn(5, 1), [{ switchSession: 3 }]);
  assert.deepEqual(h.btn(3, 0), []);
  assert.deepEqual(h.btn(50, 1), []);
  assert.deepEqual(h.btn(28, 1), []);
  assert.equal(h.state.mode, 'terminal');
});

test('terminal: ESC press opens the menu, release does nothing; effects stay empty', () => {
  const h = makeHarness();
  assert.deepEqual(h.btn(1, 1), []);
  assert.equal(h.state.mode, 'open.list');
  assert.deepEqual(h.btn(1, 0), []);
  assert.equal(h.state.mode, 'open.list');
  assert.equal(h.state.esc, 'up');
});

// ---- ESC tap vs hold -----------------------------------------------------------------
test('ESC down arms the hold timer; a release before it fires is a tap that selects', () => {
  const h = makeHarness();
  h.open();
  const down = h.escDown();
  assert.deepEqual(down, [cancel('esc-hold'), timer('esc-hold', 450)]);
  assert.equal(h.state.esc, 'armed');
  const up = h.escUp();
  assert.deepEqual(up.slice(0, 1), [cancel('esc-hold')]);
  assert.equal(h.state.esc, 'up');
  assert.equal(h.state.stack.join('/'), 'root/bluetooth', 'tap selected the first row');
});

test('ESC hold: timer fired pops one level; the release afterwards is a no-op', () => {
  const h = makeHarness();
  h.open();
  h.push(); // into Bluetooth (effects ignored)
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.stack.join('/'), 'root');
  assert.equal(h.state.esc, 'up');
  const mark = h.mark();
  h.escUp();
  assert.equal(h.effects.length, mark);
  assert.equal(h.state.stack.join('/'), 'root');
});

test('ESC hold at the root closes the menu and cancels timers', () => {
  const h = makeHarness();
  h.open();
  h.escDown();
  const fx = h.timer('esc-hold');
  assert.equal(h.state.mode, 'terminal');
  assert.deepEqual(fx, [cancel('esc-hold'), cancel('enter-guard')]);
});

test('stale esc-hold timer (menu closed meanwhile, or no press armed) is ignored', () => {
  const h = makeHarness();
  assert.deepEqual(h.timer('esc-hold'), []);
  h.open();
  assert.deepEqual(h.timer('esc-hold'), []);
  assert.equal(h.state.mode, 'open.list');
});

test('a second ESC press supersedes a leaked timer', () => {
  const h = makeHarness();
  h.open();
  h.escDown();
  const fx = h.escDown();
  assert.deepEqual(fx, [cancel('esc-hold'), timer('esc-hold', 450)]);
});

test('ESC autorepeat (value 2) is ignored inside the menu', () => {
  const h = makeHarness();
  h.open();
  h.escDown();
  assert.deepEqual(h.btn(1, 2), []);
  assert.equal(h.state.esc, 'armed');
});

test('ESC hold while the menu is closed externally (close click) leaves nothing armed', () => {
  const h = makeHarness();
  h.open();
  h.escDown();
  h.send(Events.closeClick());
  assert.equal(h.state.mode, 'terminal');
  assert.equal(h.state.esc, 'up');
  assert.deepEqual(h.timer('esc-hold'), []);
});

test('configured esc_hold_ms is used', () => {
  const h = makeHarness({ input: { esc_hold_ms: 700 } });
  h.open();
  assert.deepEqual(h.escDown(), [cancel('esc-hold'), timer('esc-hold', 700)]);
});

// ---- list navigation ---------------------------------------------------------------
test('knob moves one row per message regardless of magnitude, clamped', () => {
  const h = makeHarness();
  h.open();
  assert.deepEqual(h.view().rows.map((r) => r.label), ['Bluetooth', 'Brightness', 'Terminal', 'Timezone: ...', 'USB mode: ...', 'Debug access', 'Users']);
  h.knob(7);
  assert.equal(h.view().highlight, 1);
  h.knob(1); h.knob(1); h.knob(1); h.knob(1); h.knob(1);
  assert.equal(h.view().highlight, 6);
  h.knob(-9);
  assert.equal(h.view().highlight, 5);
  h.knob(0);
  assert.equal(h.view().highlight, 5);
});

test('KEY_1 backs out, KEY_2..4 are ignored, KEY_ENTER selects', () => {
  const h = makeHarness();
  h.open();
  h.btn(28, 1);
  assert.equal(h.view().title, 'Bluetooth');
  h.btn(3, 1);
  assert.equal(h.view().title, 'Bluetooth');
  h.btn(2, 1);
  assert.equal(h.view().title, 'Settings');
  h.btn(2, 1);
  assert.equal(h.state.mode, 'open.list');
  assert.equal(h.state.stack.length, 1);
});

test('rowTap sets the highlight and selects; row id is irrelevant', () => {
  const h = makeHarness();
  h.open();
  h.send(Events.rowTap(2));
  assert.equal(h.view().title, 'Terminal');
});

test('select on a nav row emits its enter effects once, in order, after pushing', () => {
  const h = makeHarness();
  h.open();
  h.btn(28, 1); // Bluetooth
  h.knob(1);
  const fx = h.btn(28, 1); // Pair a device...
  assert.deepEqual(fx, [{ dispatch: { type: 'local/bt_devices_reset' } }, { send: { type: 'bt_scan_start' } }]);
  assert.equal(h.view().title, 'Pair a device');
});

// ---- adjusting ----------------------------------------------------------------------
test('adjusting: first select arms, knob steps by max/20 clamped, second select disarms', () => {
  const h = makeHarness();
  h.wire({ type: 'brightness', value: 80, max: 160 });
  h.open();
  h.moveTo('Brightness');
  h.push();
  assert.equal(h.state.mode, 'open.adjusting');
  let fx = h.knob(1);
  assert.deepEqual(fx, [{ dispatch: { type: 'local/brightness', value: 88 } }, { send: { type: 'brightness_set', value: 88 } }]);
  h.settleDomain(fx);
  fx = h.knob(-100);
  assert.deepEqual(fx[1], { send: { type: 'brightness_set', value: 0 } });
  h.settleDomain(fx);
  fx = h.knob(1000);
  assert.deepEqual(fx[1], { send: { type: 'brightness_set', value: 160 } });
  h.push();
  assert.equal(h.state.mode, 'open.list');
});

test('adjusting: back and ESC hold leave adjust mode without leaving the screen or closing the menu', () => {
  const h = makeHarness();
  h.open();
  h.moveTo('Brightness');
  h.push();
  h.back();
  assert.equal(h.state.mode, 'open.list');
  h.push();
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.mode, 'open.list');
  assert.equal(h.state.stack.length, 1);
  h.push();
  h.escDown();
  h.escUp(); // tap
  assert.equal(h.state.mode, 'open.list', 'tap toggles adjust off');
});

test('adjusting never survives a navigation step', () => {
  const h = makeHarness();
  h.open();
  h.moveTo('Brightness');
  h.push();
  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA', passkey: 1 });
  assert.equal(h.state.mode, 'open.pairing.confirm');
});

test('font size range uses the configured bounds and step', () => {
  const h = makeHarness({ ui: { font_size: 20, font_size_min: 10, font_size_max: 22, font_size_step: 2 } });
  h.open();
  h.moveTo('Terminal');
  h.push();
  h.push(); // arm font size
  let fx = h.knob(1);
  assert.deepEqual(fx, [{ dispatch: { type: 'local/terminal', patch: { fontSize: 22 } } }, { term: { fontSize: 22 } }]);
  h.settleDomain(fx);
  fx = h.knob(5);
  assert.deepEqual(fx[0].dispatch.patch, { fontSize: 22 }, 'clamped at max');
});

// ---- text entry and the Enter double delivery guard -----------------------------------
function toPasswordScreen(h) {
  h.wire({ type: 'wifi_available', available: true, interface: 'wlan0' });
  h.open();
  h.choose('Wi-Fi');
  h.choose('Scan for networks...');
  h.wire({ type: 'wifi_scan_results', networks: [{ ssid: 'Cafe', signal_pct: 60, security: 'wpa2', supported: true }] });
  h.choose('Cafe');
  assert.equal(h.view().title, 'Cafe');
}

test('selecting the password field enters editingText and asks the view to focus the input', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  const fx = h.push();
  assert.equal(h.state.mode, 'open.editingText');
  assert.deepEqual(fx, [{ textBegin: { masked: true, value: '' } }]);
  assert.equal(h.view().textFieldVisible, true);
  assert.equal(h.view().hint, 'type the password \u00b7 enter or esc: done');
});

test('editingText: knob ignored, input events update the password, select/back/Enter/blur end editing', () => {
  ['select', 'back', 'enter', 'blur'].forEach((how) => {
    const h = makeHarness();
    toPasswordScreen(h);
    h.moveTo('Password');
    h.push();
    assert.deepEqual(h.knob(1), []);
    h.send(Events.textInput('hunter2hunter2'));
    assert.equal(h.state.ui.wifi.password, 'hunter2hunter2');
    const fx = how === 'select' ? h.btn(28, 1) : how === 'back' ? h.btn(2, 1) : how === 'enter' ? h.send(Events.textEnter()) : h.send(Events.textBlur());
    assert.equal(h.state.mode, 'open.swallowEnter', how);
    assert.deepEqual(fx, [timer('enter-guard', 400), { textEnd: true }], how);
  });
});

test('swallowEnter: selects are ignored until enter-guard fires, knob and back still work', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  h.send(Events.textEnter()); // ends editing, DOM Enter first
  assert.deepEqual(h.btn(28, 1), [], 'bridge button 28 within the window is swallowed');
  assert.equal(h.state.mode, 'open.swallowEnter');
  h.send(Events.rowTap(0));
  assert.equal(h.state.mode, 'open.swallowEnter', 'touch select is swallowed too');
  h.knob(1);
  assert.equal(h.view().highlight, 1);
  assert.equal(h.state.mode, 'open.swallowEnter');
  h.timer('enter-guard');
  assert.equal(h.state.mode, 'open.list');
  h.moveTo('Password');
  const fx = h.push();
  assert.equal(h.state.mode, 'open.editingText');
  assert.equal(fx.length, 1);
});

test('swallowEnter: the other order (bridge 28 ends editing, DOM Enter keydown follows) cannot reopen the field', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  h.btn(28, 1); // ends editing
  h.btn(28, 1); // second delivery inside the window
  h.send(Events.textEnter()); // stray Enter event from a no-longer-focused input
  assert.equal(h.state.mode, 'open.swallowEnter');
});

test('swallowEnter back pops and keeps swallowing; closing the menu clears it', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  h.send(Events.textEnter());
  h.back();
  assert.equal(h.view().title, 'Wi-Fi networks');
  assert.equal(h.state.mode, 'open.swallowEnter');
  h.escDown();
  h.timer('esc-hold'); // hold at depth > 1 goes back again
  h.escDown();
  h.timer('esc-hold');
  h.escDown();
  h.timer('esc-hold'); // at root with swallow: closes
  assert.equal(h.state.mode, 'terminal');
  assert.deepEqual(h.timer('enter-guard'), []);
});

test('enter_guard_ms 0 disables the swallow state', () => {
  const h = makeHarness({ input: { enter_guard_ms: 0 } });
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  const fx = h.send(Events.textEnter());
  assert.equal(h.state.mode, 'open.list');
  assert.deepEqual(fx, [{ textEnd: true }]);
});

test('leaving the password screen discards the password and resets the field', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  h.send(Events.textInput('hunter2hunter2'));
  h.send(Events.textEnter());
  h.timer('enter-guard');
  const fx = h.btn(2, 1);
  assert.equal(h.state.ui.wifi.password, '');
  assert.ok(fx.some((e) => e.textReset));
});

test('ESC hold while typing ends editing and keeps the screen', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.moveTo('Password');
  h.push();
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.mode, 'open.swallowEnter');
  assert.equal(h.view().title, 'Cafe');
});

// ---- picker -------------------------------------------------------------------------
test('picker: knob moves the ring, select applies, [OK] pops to Connect, taps work', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.choose('Pick characters');
  assert.equal(h.state.mode, 'open.picker');
  assert.equal(h.state.ui.wifi.pickerIdx, 2);
  h.knob(1);
  h.btn(28, 1);
  assert.equal(h.state.ui.wifi.password, 'b');
  h.send(Events.customTap(-1, 2)); // neighbour: move to index 2 ('a')
  assert.equal(h.state.ui.wifi.pickerIdx, 2);
  h.send(Events.customTap(0, 2)); // centre applies 'a'
  assert.equal(h.state.ui.wifi.password, 'ba');
  h.knob(-1); h.knob(-1); // [DEL] then [OK]
  h.knob(1); // back to [DEL]
  h.btn(28, 1);
  assert.equal(h.state.ui.wifi.password, 'b');
  h.knob(-1);
  h.btn(28, 1); // [OK]
  assert.equal(h.view().title, 'Cafe');
  assert.equal(h.view().highlight, 3);
  assert.equal(h.state.mode, 'open.list');
});

// ---- pairing --------------------------------------------------------------------------
test('pairing request while the menu is closed opens it on the confirm state', () => {
  const h = makeHarness();
  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA:BB', passkey: 123456 });
  assert.equal(h.state.mode, 'open.pairing.confirm');
  assert.equal(h.view().title, 'Pair with AA:BB? (123456)');
  assert.equal(h.view().hint, 'knob: move \u00b7 esc: confirm selection \u00b7 esc (hold): cancel pairing');
  h.wire({ type: 'bt_pairing_passkey_display', address: 'AA:BB', passkey: 654321 });
  assert.equal(h.state.mode, 'open.pairing.passkey');
  assert.equal(h.view().hint, 'esc (hold): cancel pairing');
});

test('pairing: Accept replies and pops; bt_pair_result pops, toasts and clears; cancel clears via onLeave', () => {
  const h = makeHarness();
  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA:BB', passkey: 1 });
  let fx = h.btn(28, 1);
  assert.deepEqual(fx, [{ send: { type: 'bt_pairing_confirm', accept: true } }]);
  assert.equal(h.state.stack.length, 1);
  assert.equal(h.state.ui.bt.pairing, null);

  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA:BB', passkey: 2 });
  fx = h.wire({ type: 'bt_pair_result', address: 'AA:BB', success: false, error: 'timed out' });
  assert.equal(h.state.stack.length, 1);
  assert.deepEqual(fx, [cancel('toast'), timer('toast', 4000)]);
  assert.equal(h.state.toast, 'Pairing failed: timed out');
  assert.equal(h.state.mode, 'open.list');

  h.wire({ type: 'bt_pairing_passkey_display', address: 'AA:BB', passkey: 3 });
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.ui.bt.pairing, null);
  assert.equal(h.state.stack.length, 1);
});

test('pairing guard: a second bt_pair is blocked while one is in flight', () => {
  const h = makeHarness();
  h.wire({ type: 'bt_scan_started' });
  h.wire({ type: 'bt_device_found', address: 'AA', name: 'Kbd' });
  h.open();
  h.push(); // Bluetooth
  h.knob(1);
  h.push(); // Pair a device (clears the device list)
  assert.deepEqual(h.push(), [], 'devices were reset by entering; nothing listed yet');
  h.wire({ type: 'bt_device_found', address: 'AA', name: 'Kbd' });
  assert.deepEqual(h.push(), [{ send: { type: 'bt_pair', address: 'AA' } }]);
  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA', passkey: 1 });
  h.push(); // Accept -> pops, onLeave clears pairing
  assert.deepEqual(h.push(), [{ send: { type: 'bt_pair', address: 'AA' } }]);
  h.wire({ type: 'bt_pairing_confirm_request', address: 'AA', passkey: 2 });
  h.escDown();
  h.timer('esc-hold'); // manual cancel
  assert.deepEqual(h.push(), [{ send: { type: 'bt_pair', address: 'AA' } }], 'cancel cleared the guard too');
});

// ---- toasts, wire reactions ---------------------------------------------------------------
test('toast: shown, restarted by a new one, cleared by the timer event', () => {
  const h = makeHarness();
  h.wire({ type: 'bt_error', context: 'x', message: 'boom' });
  assert.equal(h.state.toast, 'BT error (x): boom');
  const fx = h.wire({ type: 'settings_error', context: 'y', message: 'bad' });
  assert.deepEqual(fx, [cancel('toast'), timer('toast', 4000)]);
  assert.equal(h.state.toast, 'Settings error (y): bad');
  h.timer('toast');
  assert.equal(h.state.toast, null);
});

test('sendFailed events become toasts', () => {
  const h = makeHarness();
  h.send(Events.sendFailed('wifi_connect', 'bridge'));
  assert.equal(h.state.toast, 'Controller not connected (wifi_connect not sent)');
  h.send(Events.sendFailed('input', 'agent'));
  assert.equal(h.state.toast, 'Terminal agent not connected');
});

test('wifi_available false pops every Wi-Fi screen and clears the connecting state', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.wire({ type: 'wifi_available', available: false, interface: null });
  assert.equal(h.state.stack.join('/'), 'root');
  assert.equal(h.state.ui.wifi.password, '');
  assert.equal(h.state.mode, 'open.list');
  assert.deepEqual(h.labels(), ['Bluetooth', 'Brightness', 'Terminal', 'Timezone: ...', 'USB mode: ...', 'Debug access', 'Users']);
});

test('wifi connect result: lands on the action row, toasts only when the screen was left', () => {
  const h = makeHarness();
  toPasswordScreen(h);
  h.wire({ type: 'wifi_connect_result', ssid: 'Cafe', success: false, message: 'Wrong password' });
  assert.equal(h.state.toast, 'Wi-Fi: Wrong password', 'password screen is on top, not the connecting screen');
  const h2 = makeHarness();
  toPasswordScreen(h2);
  h2.moveTo('Show password');
  h2.send(Events.textInput('hunter2hunter2'));
  h2.state = Object.assign({}, h2.state, { ui: Object.assign({}, h2.state.ui, { wifi: Object.assign({}, h2.state.ui.wifi, { password: 'hunter2hunter2' }) }) });
  h2.choose('Connect');
  assert.equal(h2.view().title, 'Cafe');
  assert.deepEqual(h2.sentSince(0).slice(-1), [{ type: 'wifi_connect', ssid: 'Cafe', password: 'hunter2hunter2' }]);
  h2.wire({ type: 'wifi_connect_result', ssid: 'Cafe', success: true });
  assert.equal(h2.state.toast, null);
  assert.deepEqual(h2.labels(), ['Connected', 'Done']);
  assert.equal(h2.view().highlight, 1);
  assert.equal(h2.state.ui.wifi.password, '');
});

test('forgetting the selected saved network pops back to the saved list', () => {
  const h = makeHarness();
  h.wire({ type: 'wifi_available', available: true });
  h.wire({ type: 'wifi_saved_list', networks: [{ ssid: 'Office', current: false }] });
  h.open();
  h.choose('Wi-Fi');
  h.choose('Saved networks');
  h.btn(28, 1);
  h.knob(1);
  h.btn(28, 1);
  assert.equal(h.view().title, 'Forget Office?');
  h.wire({ type: 'wifi_saved_list', networks: [] });
  assert.equal(h.view().title, 'Saved networks');
});

test('paired device sync: a list without the selected device pops the action screen', () => {
  const h = makeHarness();
  h.wire({ type: 'bt_paired_list', devices: [{ address: 'AA', name: 'K', connected: true }] });
  h.open();
  h.choose('Bluetooth');
  h.choose('Paired devices');
  h.btn(28, 1);
  assert.equal(h.view().title, 'K');
  h.wire({ type: 'bt_paired_list', devices: [{ address: 'AA', name: 'K', connected: false }] });
  assert.equal(h.view().title, 'K');
  assert.deepEqual(h.labels(), ['Connect', 'Forget']);
  h.wire({ type: 'bt_paired_list', devices: [] });
  assert.equal(h.view().title, 'Paired devices');
});

test('timezone: list requested once, groups, zones, set returns to the root', () => {
  const h = makeHarness();
  h.open();
  const fx = h.choose('Timezone');
  assert.deepEqual(fx.filter((e) => e.send), [{ send: { type: 'timezone_list_get' } }]);
  h.wire({ type: 'timezone_list', zones: ['Europe/London', 'UTC', 'America/Chicago'] });
  assert.deepEqual(h.labels(), ['America', 'Europe', 'UTC']);
  h.choose('Europe');
  assert.equal(h.view().title, 'Europe');
  const fx2 = h.choose('Europe/London');
  assert.deepEqual(fx2, [{ send: { type: 'timezone_set', value: 'Europe/London' } }]);
  assert.equal(h.state.stack.length, 1);
  const fx3 = h.choose('Timezone');
  assert.deepEqual(fx3.filter((e) => e.send), [], 'list already loaded');
});

test('every screen has a title and a known row kind; hints never use screen ids in menu-view', () => {
  const h = makeHarness();
  const kinds = new Set(['label', 'toggle', 'range', 'nav', 'nav-dynamic', 'field', 'action']);
  const ctx = (top) => ({ domain: h.domain, ui: h.state.ui, stack: ['root', top], top, highlight: {}, cfg: h.cfg });
  Object.keys(h.screens.defs).forEach((id) => {
    const def = h.screens.defs[id];
    assert.ok(def.title !== undefined, id);
    if (def.rows) def.rows(ctx(id)).forEach((r) => assert.ok(kinds.has(r.kind), id + ' ' + r.kind));
  });
  const src = require('node:fs').readFileSync(require.resolve('../src/menu/menu-view.js'), 'utf8');
  assert.ok(!/wifi|bt-|bluetooth/i.test(src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/wifi-bars/g, '')), 'generic view must not know feature screens');
  const chartSrc = require('node:fs').readFileSync(require.resolve('../src/interaction/menu-chart.js'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  assert.ok(!/wifi|bluetooth|bt-/i.test(chartSrc), 'chart must not know feature screens');
});

// ---- disabled rows -------------------------------------------------------------------
const Chart = require('../src/interaction/menu-chart.js');

function disabledChart(extra) {
  const rows = [
    { id: 'ok', label: 'Ok', kind: 'action', afterSelect: 'none', activate: [{ send: { type: 'ok' } }] },
    Object.assign({ id: 'off', label: 'Off', kind: 'action', afterSelect: 'none', activate: [{ send: { type: 'off' } }], disabled: true, disabledHint: 'Unlock first' }, extra || {}),
    { id: 'tg', label: 'Tg', kind: 'toggle', display: 'OFF', activate: [{ send: { type: 'tg' } }], disabled: (ctx) => !ctx.domain.unlocked, disabledHint: (ctx) => 'locked ' + ctx.top },
    { id: 'rb', label: 'Reboot', kind: 'action', hold: true, activate: [{ send: { type: 'reboot' } }], disabled: true },
    { id: 'rb2', label: 'Reboot2', kind: 'action', hold: true, activate: [{ send: { type: 'reboot2' } }] },
  ];
  const screens = { defs: { root: { title: 'T', rows: () => rows } }, reactions: {}, initialUi: () => ({}) };
  const cfg = { input: { esc_hold_ms: 450, enter_guard_ms: 0 }, ui: { toast_ms: 2000 }, terminal: { knob_scroll_lines: 2 } };
  const chart = Chart.create({ screens, cfg });
  let state = Object.assign(chart.initial({ unlocked: false }), { mode: 'open.list' });
  const h = {
    fx(ev) { const r = chart.transition(state, ev); state = r.state; return r.effects; },
    get state() { return state; },
  };
  h.knob = (d) => h.fx(Events.knob(d));
  h.btn = (c, v) => h.fx(Events.button(c, v));
  return h;
}

test('disabled: select is a no-op that toasts the hint; the knob can still land on the row', () => {
  const h = disabledChart();
  h.knob(1);
  assert.equal(h.state.highlight.root, 1);
  const fx = h.btn(28, 1);
  assert.deepEqual(fx, [cancel('toast'), timer('toast', 2000)]);
  assert.equal(h.state.toast, 'Unlock first');
  assert.equal(fx.some((e) => e.send), false);
});

test('disabled without a hint: select does nothing at all', () => {
  const h = disabledChart({ disabledHint: undefined });
  h.knob(1);
  assert.deepEqual(h.btn(28, 1), []);
  assert.equal(h.state.toast, null);
});

test('disabled inline toggle (function of ctx, function hint) does not send; enabled rows are unchanged', () => {
  const h = disabledChart();
  h.knob(1); h.knob(1);
  const fx = h.btn(28, 1);
  assert.equal(fx.some((e) => e.send), false);
  assert.equal(h.state.toast, 'locked root');
  // Unlocked via the domain: the same row now sends.
  h.fx(Events.domain({ unlocked: true }));
  assert.deepEqual(h.btn(28, 1), [{ send: { type: 'tg' } }]);
  // And the first, never-disabled row is untouched.
  h.knob(-1); h.knob(-1);
  assert.deepEqual(h.btn(28, 1), [{ send: { type: 'ok' } }]);
});

test('disabled hold row never arms: no enter-hold timer, a stale one does not fire, esc tap only hints', () => {
  const h = disabledChart();
  h.knob(1); h.knob(1); h.knob(1);
  const down = h.btn(28, 1);
  assert.equal(down.some((e) => e.startTimer && e.startTimer.name === 'enter-hold'), false);
  assert.equal(h.state.enter == null, true);
  assert.equal(h.fx(Events.timerFired('enter-hold')).some((e) => e.send), false);
  h.btn(28, 0);
  h.btn(1, 1);
  const tap = h.btn(1, 0);
  assert.equal(tap.some((e) => e.send), false);
  assert.equal(h.state.toast, null);
});

test('enabled hold row still arms and fires on the timer', () => {
  const h = disabledChart();
  for (let i = 0; i < 4; i++) h.knob(1);
  assert.deepEqual(h.btn(28, 1), [cancel('enter-hold'), timer('enter-hold', 450)]);
  assert.deepEqual(h.fx(Events.timerFired('enter-hold')), [{ send: { type: 'reboot2' } }]);
});

test('disabled row: a tap goes through the same select and is a no-op with a hint', () => {
  const h = disabledChart();
  const fx = h.fx(Events.rowTap(1));
  assert.equal(fx.some((e) => e.send), false);
  assert.equal(h.state.toast, 'Unlock first');
});
