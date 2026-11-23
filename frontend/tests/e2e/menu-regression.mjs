// Headless-Chromium regression net for the kiosk UI (800x480, mocked
// WebSockets). Drives index.html over file:// exactly like the device does,
// pushes controller/agent frames through the in-page mock and asserts on the
// DOM and on the frames the app sends back. Every refactor step must keep it
// green; the assertions record the behavior of the original monolithic frontend.
//
//   node frontend/tests/e2e/menu-regression.mjs
//
// Skips (exit 0, clear message) when playwright or a Chromium is not found;
// see mock-server.mjs for how they are resolved. No npm dependency is added.
import assert from 'node:assert/strict';
import { resolvePlaywright, resolveChrome, MockServer, MOCK_WS_SOURCE, PAGE_URL, BRIDGE, AGENT, fixture } from './mock-server.mjs';

const pw = resolvePlaywright();
const chromePath = resolveChrome();
if (!pw || !chromePath) {
  console.log('SKIP e2e menu regression: ' + (!pw ? 'playwright module not found (set PLAYWRIGHT_MODULE)' : 'no Chromium binary found (set CHROME_PATH)'));
  process.exit(0);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let passed = 0;
let failed = 0;
const failures = [];

async function check(name, fn) {
  try {
    await fn();
    passed++;
    if (process.env.E2E_VERBOSE) console.log('  ok   ' + name);
  } catch (e) {
    failed++;
    failures.push(name + ': ' + (e && e.message ? e.message.split('\n').slice(0, 6).join(' | ') : e));
    console.log('  FAIL ' + name + '\n       ' + (e && e.message ? e.message.split('\n').slice(0, 8).join('\n       ') : e));
  }
}

const browser = await pw.chromium.launch({ executablePath: chromePath, headless: true, args: ['--no-sandbox'] });

// ---- per-scenario page -----------------------------------------------------
async function scenario(name, fn, opts) {
  console.log('scenario: ' + name);
  const ctx = await browser.newContext({ viewport: { width: 800, height: 480 } });
  await ctx.addInitScript(MOCK_WS_SOURCE);
  const page = await ctx.newPage();
  const pageErrors = [];
  page.on('pageerror', (e) => pageErrors.push(String(e)));
  page.on('console', (m) => { if (m.type() === 'error') pageErrors.push('console.error: ' + m.text()); });
  const srv = new MockServer(page);
  await page.goto(PAGE_URL);
  await page.waitForFunction(() => document.getElementById('agent-text').textContent === 'AGENT: LIVE' &&
    document.getElementById('bridge-text').textContent === 'BRIDGE: LIVE', null, { timeout: 5000 });
  const h = makeHelpers(page, srv);
  try {
    await fn(h, page, srv);
  } catch (e) {
    failed++;
    failures.push(name + ' (aborted): ' + e.message.split('\n')[0]);
    console.log('  FAIL scenario aborted: ' + e.message.split('\n').slice(0, 6).join('\n       '));
  }
  await check(name + ': no page errors', () => assert.deepEqual(pageErrors, []));
  await ctx.close();
}

function makeHelpers(page, srv) {
  const h = {};
  h.view = () => page.evaluate(() => {
    const txt = (el) => (el ? el.textContent : '');
    const rows = Array.from(document.querySelectorAll('#menu-list .menu-row')).map((r) => ({
      label: txt(r.querySelector('.menu-row-label')),
      value: Array.from(r.querySelectorAll('.menu-row-value')).map(txt).join(' '),
      sel: r.classList.contains('sel'),
      adj: r.classList.contains('adjusting'),
      dot: r.querySelector('.menu-row-dot') ? r.querySelector('.menu-row-dot').classList.contains('ok') : null,
      bars: r.querySelectorAll('.wifi-bars i.on').length || null,
    }));
    const toast = document.getElementById('menu-toast');
    return {
      open: !document.getElementById('menu-overlay').classList.contains('hidden'),
      title: txt(document.getElementById('menu-title')),
      rows,
      sel: rows.findIndex((r) => r.sel),
      hint: txt(document.getElementById('menu-hint')),
      toast: toast.classList.contains('hidden') ? null : toast.textContent,
      inputShown: !document.getElementById('wifi-input-wrap').classList.contains('hidden'),
      inputFocused: document.activeElement && document.activeElement.id === 'wifi-pass-input',
      inputValue: document.getElementById('wifi-pass-input').value,
    };
  });
  h.labels = async () => (await h.view()).rows.map((r) => r.label + (r.value ? '|' + r.value : ''));
  h.bridge = (m) => srv.bridge(m);
  h.agent = (m) => srv.agent(m);
  h.btn = (code, value) => srv.bridge({ type: 'button', code, value });
  h.knob = (delta) => srv.bridge({ type: 'knob', delta });
  h.press = async (code) => { await h.btn(code, 1); await h.btn(code, 0); await sleep(30); };
  h.push = () => h.press(28); // knob push
  h.back = () => h.press(2); // KEY_1
  h.esc = async () => { await h.press(1); await sleep(30); }; // open (menu closed) on press
  h.escTap = async () => { await h.btn(1, 1); await sleep(80); await h.btn(1, 0); await sleep(30); };
  h.escHold = async () => { await h.btn(1, 1); await sleep(650); await h.btn(1, 0); await sleep(30); };
  h.moveTo = async (label) => {
    for (let i = 0; i < 40; i++) {
      const v = await h.view();
      const idx = v.rows.findIndex((r) => r.label === label || r.label.indexOf(label) === 0);
      if (idx < 0) throw new Error('row not found: ' + label + ' in ' + JSON.stringify(v.rows.map((r) => r.label)));
      if (idx === v.sel) return;
      await h.knob(idx > v.sel ? 1 : -1);
    }
    throw new Error('moveTo did not converge for ' + label);
  };
  h.choose = async (label) => { await h.moveTo(label); await h.push(); };
  h.openMenu = async () => { await h.esc(); };
  h.sentSince = (mark, url) => srv.sentSince(mark, url || BRIDGE);
  h.mark = (url) => srv.sentCount(url || BRIDGE);
  // xterm.write() is asynchronous, so let it flush before reading the screen
  h.text = async () => { await sleep(150); return page.evaluate(() => document.getElementById('screen').innerText); };
  h.statusText = () => page.evaluate(() => ({
    agent: document.getElementById('agent-text').textContent,
    bridge: document.getElementById('bridge-text').textContent,
    agentOk: document.getElementById('agent-dot').classList.contains('ok'),
    bridgeOk: document.getElementById('bridge-dot').classList.contains('ok'),
    session: document.getElementById('session-text').textContent,
  }));
  h.tabs = () => page.evaluate(() => Array.from(document.querySelectorAll('#tab-bar .tab')).map((t) => ({ name: t.textContent, active: t.classList.contains('active') })));
  return h;
}

const V = {
  rootHint: 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): close',
  subHint: 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): back',
  adjustHint: 'knob: adjust \u00b7 esc: confirm \u00b7 esc (hold): cancel',
};
const bt = (code, value) => ({ type: 'button', code, value });

// =============================================================================
await scenario('boot, tabs, output, keyboard', async (h, page, srv) => {
  await check('status bar shows both links live', async () => {
    const s = await h.statusText();
    assert.equal(s.agent, 'AGENT: LIVE');
    assert.equal(s.bridge, 'BRIDGE: LIVE');
    assert.equal(s.agentOk && s.bridgeOk, true);
  });
  await check('bridge getters sent on connect, in order', async () => {
    assert.deepEqual(await h.sentSince(0), [{ type: 'bt_power_get' }, { type: 'brightness_get' }, { type: 'timezone_get' }, { type: 'wifi_get' }, { type: 'user_bootstrap_get' }]);
  });
  await check('agent gets a resize for session 0 with fitted size', async () => {
    const msgs = await h.sentSince(0, AGENT);
    const r = msgs.filter((m) => m.type === 'resize')[0];
    assert.ok(r, 'no resize sent');
    assert.equal(r.session, 0);
    assert.ok(r.cols > 10 && r.rows > 3, JSON.stringify(r));
  });
  await h.agent(fixture('terminal/server_session_list'));
  await check('session_list renders tabs and session label', async () => {
    assert.deepEqual(await h.tabs(), [{ name: 'shell-1', active: true }, { name: 'shell-3', active: false }]);
    assert.equal((await h.statusText()).session, 'session: shell-1');
  });
  await h.agent(fixture('terminal/server_output', { session: 0, data: 'hello-zero\r\n' }));
  await check('output for the active session is rendered by xterm', async () => {
    assert.match(await h.text(), /hello-zero/);
  });
  await h.agent(fixture('terminal/server_output', { session: 2, data: 'hidden-two\r\n' }));
  await check('output for a background session is not rendered', async () => {
    assert.doesNotMatch(await h.text(), /hidden-two/);
  });
  await check('keyboard: printable, Enter, Ctrl+C, arrows go to the active session as input', async () => {
    const mark = await h.mark(AGENT);
    await page.keyboard.press('a');
    await page.keyboard.press('Enter');
    await page.keyboard.press('Control+c');
    await page.keyboard.press('ArrowUp');
    await page.keyboard.press('Backspace');
    assert.deepEqual(await h.sentSince(mark, AGENT), [
      { type: 'input', session: 0, data: 'a' },
      { type: 'input', session: 0, data: '\r' },
      { type: 'input', session: 0, data: '\x03' },
      { type: 'input', session: 0, data: '\x1b[A' },
      { type: 'input', session: 0, data: '\x7f' },
    ]);
  });
  await check('knob in terminal mode scrolls instead of opening the menu', async () => {
    await h.agent(fixture('terminal/server_output', { session: 0, data: Array.from({ length: 80 }, (_, i) => 'line' + i).join('\r\n') + '\r\n' }));
    await page.waitForFunction(() => document.querySelector('.xterm-viewport').scrollTop > 100, null, { timeout: 3000 });
    const before = await page.evaluate(() => document.querySelector('.xterm-viewport').scrollTop);
    await h.knob(-3);
    await sleep(100);
    const after = await page.evaluate(() => document.querySelector('.xterm-viewport').scrollTop);
    assert.ok(after < before, 'scrollTop ' + before + ' -> ' + after);
    assert.equal((await h.view()).open, false);
  });
});

// =============================================================================
await scenario('menu open/close, knob navigation, ESC tap vs hold', async (h, page, srv) => {
  await h.openMenu();
  await check('ESC press opens the menu at the root', async () => {
    const v = await h.view();
    assert.equal(v.open, true);
    assert.equal(v.title, 'Settings');
    assert.deepEqual(v.rows.map((r) => r.label), ['Bluetooth', 'Brightness', 'Terminal', 'Timezone: ...', 'Users']);
    assert.equal(v.sel, 0);
    assert.equal(v.hint, V.rootHint);
  });
  await check('knob moves the highlight one row per message, clamped at both ends', async () => {
    await h.knob(5); assert.equal((await h.view()).sel, 1);
    await h.knob(1); await h.knob(1); await h.knob(1); await h.knob(1); await h.knob(1);
    assert.equal((await h.view()).sel, 4);
    await h.knob(-9); assert.equal((await h.view()).sel, 3);
    await h.knob(-1); await h.knob(-1); await h.knob(-1);
    assert.equal((await h.view()).sel, 0);
  });
  await check('keyboard keystrokes while the menu is open are not typed into the PTY', async () => {
    const mark = await h.mark(AGENT);
    await page.keyboard.press('x');
    await page.keyboard.press('Enter');
    assert.deepEqual(await h.sentSince(mark, AGENT), []);
  });
  await check('buttons 2..4 (KEY_2..KEY_4) are ignored while the menu is open', async () => {
    await h.agent(fixture('terminal/server_session_list'));
    await h.press(4);
    assert.equal((await h.tabs()).filter((t) => t.active)[0].name, 'shell-1');
  });
  await check('ESC tap confirms (selects Bluetooth)', async () => {
    await h.escTap();
    const v = await h.view();
    assert.equal(v.title, 'Bluetooth');
    assert.equal(v.hint, V.subHint);
  });
  await check('ESC hold goes back one level, release after the hold does nothing', async () => {
    const mark = await h.mark();
    await h.escHold();
    const v = await h.view();
    assert.equal(v.title, 'Settings');
    assert.deepEqual(await h.sentSince(mark), []);
  });
  await check('ESC hold at the root closes the menu', async () => {
    await h.escHold();
    assert.equal((await h.view()).open, false);
  });
  await check('ESC press with the menu closed opens, and its release is a no-op', async () => {
    await h.btn(1, 1);
    assert.equal((await h.view()).open, true);
    await h.btn(1, 0);
    await sleep(600);
    assert.equal((await h.view()).open, true);
    assert.equal((await h.view()).title, 'Settings');
  });
  await check('KEY_1 backs out of a sub-screen and does nothing at the root', async () => {
    await h.choose('Terminal');
    assert.equal((await h.view()).title, 'Terminal');
    await h.back();
    assert.equal((await h.view()).title, 'Settings');
    await h.back();
    assert.equal((await h.view()).open, true);
  });
  await check('KEY_ENTER (knob push) selects like an ESC tap', async () => {
    await h.moveTo('Bluetooth');
    await h.push();
    assert.equal((await h.view()).title, 'Bluetooth');
  });
  await check('close button (x) closes the menu and unwinds sub-screens', async () => {
    await page.evaluate(() => document.getElementById('menu-close').click());
    assert.equal((await h.view()).open, false);
    await h.esc();
    assert.equal((await h.view()).title, 'Settings');
  });
  await check('touch: clicking a row selects it', async () => {
    await page.evaluate(() => document.querySelectorAll('#menu-list .menu-row')[2].click());
    assert.equal((await h.view()).title, 'Terminal');
  });
});

// =============================================================================
await scenario('Bluetooth screens, pairing flow', async (h, page, srv) => {
  await h.bridge(fixture('controller/event.bt_power', { powered: true }));
  await h.openMenu();
  await h.choose('Bluetooth');
  await check('Bluetooth screen rows and power state', async () => {
    const v = await h.view();
    assert.equal(v.title, 'Bluetooth');
    assert.deepEqual(await h.labels(), ['Power|ON', 'Pair a device...', 'Paired devices']);
  });
  await check('selecting Power sends bt_set_power with the inverted value', async () => {
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_set_power', powered: false }]);
    await h.bridge(fixture('controller/event.bt_power', { powered: false }));
    assert.deepEqual((await h.labels())[0], 'Power|OFF');
  });
  await check('Paired devices: requests the list, shows Loading..., then rows with connection state', async () => {
    const mark = await h.mark();
    await h.choose('Paired devices');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_paired_list_get' }]);
    assert.equal((await h.view()).title, 'Paired devices');
    assert.deepEqual(await h.labels(), ['Loading...']);
    await h.bridge(fixture('controller/event.bt_paired_list'));
    const v = await h.view();
    assert.deepEqual(await h.labels(), ['Keyboard|Connected', '11:22:33:44:55:66|Disconnected']);
    assert.equal(v.rows[0].dot, true);
    assert.equal(v.rows[1].dot, false);
    assert.equal(v.hint, 'knob: move \u00b7 esc: open device \u00b7 esc (hold): back');
  });
  await check('paired device actions: Disconnect sends bt_disconnect; list update without the device pops back', async () => {
    await h.push();
    let v = await h.view();
    assert.equal(v.title, 'Keyboard');
    assert.deepEqual(await h.labels(), ['Disconnect', 'Forget']);
    assert.equal(v.hint, 'knob: move \u00b7 esc: confirm action \u00b7 esc (hold): back');
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_disconnect', address: 'AA:BB:CC:DD:EE:FF' }]);
    assert.equal((await h.view()).title, 'Keyboard');
    await h.bridge(fixture('controller/event.bt_paired_list', { devices: [{ address: '11:22:33:44:55:66', name: null, connected: false }] }));
    v = await h.view();
    assert.equal(v.title, 'Paired devices');
    assert.deepEqual(await h.labels(), ['11:22:33:44:55:66|Disconnected']);
  });
  await check('disconnected device offers Connect then Forget', async () => {
    await h.push();
    assert.deepEqual(await h.labels(), ['Connect', 'Forget']);
    const mark = await h.mark();
    await h.push();
    await h.knob(1); await h.push();
    assert.deepEqual(await h.sentSince(mark), [
      { type: 'bt_connect', address: '11:22:33:44:55:66' },
      { type: 'bt_forget', address: '11:22:33:44:55:66' },
    ]);
    await h.back(); await h.back();
    assert.equal((await h.view()).title, 'Bluetooth');
  });
  await check('Pair a device: starts a scan, lists found devices, selecting sends bt_pair', async () => {
    const mark = await h.mark();
    await h.choose('Pair a device...');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_scan_start' }]);
    assert.equal((await h.view()).title, 'Pair a device');
    assert.deepEqual(await h.labels(), ['Scanning...']);
    await h.bridge(fixture('controller/event.bt_scan_started'));
    await h.bridge(fixture('controller/event.bt_device_found'));
    await h.bridge(fixture('controller/event.bt_device_found')); // duplicate address is ignored
    assert.deepEqual(await h.labels(), ['Keyboard']);
    const m2 = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(m2), [{ type: 'bt_pair', address: 'AA:BB:CC:DD:EE:FF' }]);
  });
  await check('pairing confirm request pushes the confirm screen; Accept replies and pops', async () => {
    await h.bridge(fixture('controller/event.bt_pairing_confirm_request', { passkey: 123456 }));
    const v = await h.view();
    assert.equal(v.title, 'Pair with AA:BB:CC:DD:EE:FF? (123456)');
    assert.deepEqual(await h.labels(), ['Accept', 'Deny']);
    assert.equal(v.hint, 'knob: move \u00b7 esc: confirm selection \u00b7 esc (hold): cancel pairing');
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_pairing_confirm', accept: true }]);
    assert.equal((await h.view()).title, 'Pair a device');
  });
  await check('a second bt_pair is allowed again after the confirm screen is left', async () => {
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_pair', address: 'AA:BB:CC:DD:EE:FF' }]);
  });
  await check('bt_pair_result pops the pairing screen and toasts', async () => {
    await h.bridge(fixture('controller/event.bt_pairing_confirm_request', { passkey: 111111 }));
    assert.equal((await h.view()).title, 'Pair with AA:BB:CC:DD:EE:FF? (111111)');
    await h.bridge(fixture('controller/event.bt_pair_result', { success: true }));
    const v = await h.view();
    assert.equal(v.title, 'Pair a device');
    assert.equal(v.toast, 'Paired with AA:BB:CC:DD:EE:FF');
  });
  await check('failed pairing toasts the error text', async () => {
    await h.bridge(fixture('controller/event.bt_pair_result', { success: false, error: 'confirmation timed out' }));
    assert.equal((await h.view()).toast, 'Pairing failed: confirmation timed out');
  });
  await check('bt_error toasts with its context', async () => {
    await h.bridge(fixture('controller/event.bt_error'));
    assert.equal((await h.view()).toast, 'BT error (bt_power_get): Bluetooth unavailable');
  });
  await check('toast disappears after about 4 seconds', async () => {
    await page.waitForFunction(() => document.getElementById('menu-toast').classList.contains('hidden'), null, { timeout: 5500 });
  });
});

await scenario('pairing request while the menu is closed', async (h, page, srv) => {
  await check('confirm request opens the menu on the confirm screen', async () => {
    assert.equal((await h.view()).open, false);
    await h.bridge(fixture('controller/event.bt_pairing_confirm_request', { passkey: 424242 }));
    const v = await h.view();
    assert.equal(v.open, true);
    assert.equal(v.title, 'Pair with AA:BB:CC:DD:EE:FF? (424242)');
    assert.deepEqual(await h.labels(), ['Accept', 'Deny']);
  });
  await check('Deny sends accept:false and pops to the root', async () => {
    await h.knob(1);
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_pairing_confirm', accept: false }]);
    assert.equal((await h.view()).title, 'Settings');
  });
  await check('passkey display screen has no choices and ESC hold cancels it', async () => {
    await h.bridge(fixture('controller/event.bt_pairing_passkey_display', { passkey: 654321 }));
    const v = await h.view();
    assert.equal(v.title, 'Pair with AA:BB:CC:DD:EE:FF');
    assert.deepEqual(await h.labels(), ['Type 654321 on the keyboard, then press Enter', 'Waiting for device...']);
    assert.equal(v.hint, 'esc (hold): cancel pairing');
    await h.escHold();
    assert.equal((await h.view()).title, 'Settings');
  });
  await check('bt_pair_result closes the confirm screen opened while the menu was closed', async () => {
    await h.bridge(fixture('controller/event.bt_pairing_confirm_request', { passkey: 1 }));
    await h.bridge(fixture('controller/event.bt_pair_result', { success: true }));
    const v = await h.view();
    assert.equal(v.title, 'Settings');
    assert.equal(v.open, true);
  });
  await check('closing the menu with ESC hold at the root works afterwards', async () => {
    await h.escHold();
    assert.equal((await h.view()).open, false);
  });
});

// =============================================================================
await scenario('brightness adjust', async (h, page, srv) => {
  await h.bridge(fixture('controller/event.brightness', { value: 80, max: 160 }));
  await h.openMenu();
  await check('brightness row shows a percentage', async () => {
    assert.deepEqual((await h.labels())[1], 'Brightness|50%');
  });
  await h.moveTo('Brightness');
  await check('first push arms adjust mode (row highlighted as adjusting, hint changes)', async () => {
    await h.push();
    const v = await h.view();
    assert.equal(v.rows[1].adj, true);
    assert.equal(v.hint, V.adjustHint);
  });
  await check('knob steps by max/20 (8), sends brightness_set live, clamps at 0 and max', async () => {
    const mark = await h.mark();
    await h.knob(1);
    await h.knob(-3);
    await h.knob(100);
    await h.knob(-100);
    await h.knob(1);
    assert.deepEqual(await h.sentSince(mark), [
      { type: 'brightness_set', value: 88 },
      { type: 'brightness_set', value: 64 },
      { type: 'brightness_set', value: 160 },
      { type: 'brightness_set', value: 0 },
      { type: 'brightness_set', value: 8 },
    ]);
    assert.equal((await h.labels())[1], 'Brightness|5%');
    assert.equal((await h.view()).sel, 1, 'highlight stays put while adjusting');
  });
  await check('second push disarms; knob moves the highlight again', async () => {
    await h.push();
    let v = await h.view();
    assert.equal(v.rows[1].adj, false);
    assert.equal(v.hint, V.rootHint);
    await h.knob(1);
    assert.equal((await h.view()).sel, 2);
  });
  await check('ESC tap while adjusting disarms; ESC hold while adjusting also leaves adjust but keeps the menu open', async () => {
    await h.moveTo('Brightness');
    await h.push();
    assert.equal((await h.view()).rows[1].adj, true);
    await h.escTap();
    assert.equal((await h.view()).rows[1].adj, false);
    await h.push();
    await h.escHold();
    const v = await h.view();
    assert.equal(v.rows[1].adj, false);
    assert.equal(v.open, true);
    assert.equal(v.title, 'Settings');
  });
  await check('KEY_1 while adjusting leaves adjust mode first', async () => {
    await h.push();
    assert.equal((await h.view()).rows[1].adj, true);
    await h.back();
    assert.equal((await h.view()).rows[1].adj, false);
  });
  await check('settings_error toasts', async () => {
    await h.bridge(fixture('controller/event.settings_error'));
    assert.equal((await h.view()).toast, 'Settings error (brightness_set): brightness 200 out of range 0..=160');
  });
});

// =============================================================================
await scenario('timezone', async (h, page, srv) => {
  await h.bridge(fixture('controller/event.timezone', { value: 'Europe/Lisbon' }));
  await h.openMenu();
  await check('root row shows the current zone', async () => {
    assert.equal((await h.labels())[3], 'Timezone: Europe/Lisbon');
  });
  await check('entering requests the list once and shows Loading...', async () => {
    const mark = await h.mark();
    await h.choose('Timezone');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'timezone_list_get' }]);
    assert.equal((await h.view()).title, 'Timezone region');
    assert.deepEqual(await h.labels(), ['Loading...']);
  });
  await check('regions are grouped and sorted; bare names are leaf rows', async () => {
    await h.bridge(fixture('controller/event.timezone_list', { zones: ['Europe/Lisbon', 'Europe/London', 'UTC', 'America/New_York', 'America/Chicago'] }));
    assert.deepEqual(await h.labels(), ['America', 'Europe', 'UTC']);
  });
  await check('region screen lists sorted zones; picking one sends timezone_set and returns to the root', async () => {
    await h.choose('Europe');
    const v = await h.view();
    assert.equal(v.title, 'Europe');
    assert.deepEqual(await h.labels(), ['Europe/Lisbon', 'Europe/London']);
    const mark = await h.mark();
    await h.choose('Europe/London');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'timezone_set', value: 'Europe/London' }]);
    assert.equal((await h.view()).title, 'Settings');
  });
  await check('second visit does not re-request the list; a bare zone sets directly', async () => {
    const mark = await h.mark();
    await h.choose('Timezone');
    assert.deepEqual(await h.sentSince(mark), []);
    const m2 = await h.mark();
    await h.choose('UTC');
    assert.deepEqual(await h.sentSince(m2), [{ type: 'timezone_set', value: 'UTC' }]);
    assert.equal((await h.view()).title, 'Settings');
  });
});

// =============================================================================
await scenario('terminal font size and family', async (h, page, srv) => {
  await h.openMenu();
  await h.choose('Terminal');
  await check('terminal screen rows', async () => {
    assert.equal((await h.view()).title, 'Terminal');
    assert.deepEqual(await h.labels(), ['Font size|13', 'Font family: JetBrains Mono']);
  });
  await check('font size adjusts live, clamps, applies to xterm and re-sends a resize', async () => {
    await h.push();
    const mark = await h.mark(AGENT);
    await h.knob(1); await h.knob(1);
    assert.equal((await h.labels())[0], 'Font size|15');
    await sleep(100);
    const px = await page.evaluate(() => getComputedStyle(document.querySelector('.xterm-rows')).fontSize);
    assert.equal(px, '15px');
    const msgs = (await h.sentSince(mark, AGENT)).filter((m) => m.type === 'resize');
    assert.ok(msgs.length >= 2, 'resize per step, got ' + msgs.length);
    await h.knob(-50);
    assert.equal((await h.labels())[0], 'Font size|8');
    await h.knob(50);
    assert.equal((await h.labels())[0], 'Font size|28');
    await h.knob(-15);
    await h.push();
  });
  await check('font family list: choosing pops back and updates the label and xterm', async () => {
    await h.choose('Font family');
    const v = await h.view();
    assert.equal(v.title, 'Font family');
    assert.deepEqual(await h.labels(), ['JetBrains Mono', 'Consolas', 'Courier New', 'monospace']);
    await h.choose('Courier New');
    assert.equal((await h.view()).title, 'Terminal');
    assert.equal((await h.labels())[1], 'Font family: Courier New');
    await sleep(100);
    const ff = await page.evaluate(() => getComputedStyle(document.querySelector('.xterm-rows')).fontFamily);
    assert.match(ff, /Courier New/);
  });
});

// =============================================================================
await scenario('Wi-Fi: list, saved, connect failure and success', async (h, page, srv) => {
  await check('Wi-Fi row appears only after wifi_available, with the ssid once connected', async () => {
    await h.openMenu();
    assert.deepEqual((await h.labels()).map((l) => l.split('|')[0]), ['Bluetooth', 'Brightness', 'Terminal', 'Timezone: ...', 'Users']);
    await h.bridge(fixture('controller/event.wifi_available'));
    assert.equal((await h.labels())[1], 'Wi-Fi');
    await h.bridge(fixture('controller/event.wifi_status'));
    assert.equal((await h.labels())[1], 'Wi-Fi: Home');
  });
  await check('Wi-Fi home: requests status and saved list, shows the status block and actions', async () => {
    const mark = await h.mark();
    await h.choose('Wi-Fi');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_get' }, { type: 'wifi_saved_list_get' }]);
    const v = await h.view();
    assert.equal(v.title, 'Wi-Fi');
    assert.deepEqual(await h.labels(), [
      'Status: Connected', 'Network: Home', 'IP: 192.168.1.9', 'Signal: 80%', 'Interface: wlan0', 'Ethernet: also up',
      'Scan for networks...', 'Saved networks', 'Disconnect',
    ]);
    assert.equal(v.rows[3].bars, 4);
  });
  await check('Disconnect sends wifi_disconnect and stays on the screen', async () => {
    const mark = await h.mark();
    await h.choose('Disconnect');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_disconnect' }]);
    assert.equal((await h.view()).title, 'Wi-Fi');
  });
  await check('Saved networks: list with current marker; actions; forget confirm; list update pops back', async () => {
    const mark = await h.mark();
    await h.choose('Saved networks');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_saved_list_get' }]);
    assert.deepEqual(await h.labels(), ['Loading...']);
    await h.bridge(fixture('controller/event.wifi_saved_list'));
    let v = await h.view();
    assert.equal(v.title, 'Saved networks');
    assert.deepEqual(await h.labels(), ['Home|Connected', 'Office|Disconnected']);
    await h.knob(1); await h.push();
    v = await h.view();
    assert.equal(v.title, 'Office');
    assert.deepEqual(await h.labels(), ['Connect', 'Forget...']);
    await h.knob(1); await h.push();
    v = await h.view();
    assert.equal(v.title, 'Forget Office?');
    assert.deepEqual(await h.labels(), ['Forget this network', 'Cancel']);
    const m2 = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(m2), [{ type: 'wifi_forget', ssid: 'Office' }]);
    assert.equal((await h.view()).title, 'Forget Office?');
    await h.bridge(fixture('controller/event.wifi_saved_list', { networks: [{ ssid: 'Home', current: true }] }));
    v = await h.view();
    assert.equal(v.title, 'Saved networks');
    assert.deepEqual(await h.labels(), ['Home|Connected']);
  });
  await check('Cancel on the forget confirm pops one level', async () => {
    await h.bridge(fixture('controller/event.wifi_saved_list'));
    await h.knob(1); await h.push(); await h.knob(1); await h.push();
    assert.equal((await h.view()).title, 'Forget Office?');
    await h.knob(1); await h.push();
    assert.equal((await h.view()).title, 'Office');
  });
  await check('saved Connect sends wifi_connect_saved and shows the connecting screen; failure shows the message and Back', async () => {
    const mark = await h.mark();
    await h.choose('Connect');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_connect_saved', ssid: 'Office' }]);
    let v = await h.view();
    assert.equal(v.title, 'Office');
    assert.deepEqual(await h.labels(), ['Connecting to Office...']);
    assert.equal(v.hint, 'esc (hold): back (connection continues in background)');
    await h.bridge(fixture('controller/event.wifi_connecting', { ssid: 'Office' }));
    await h.bridge(fixture('controller/event.wifi_connect_result.failed', { ssid: 'Office', message: 'Wrong password' }));
    v = await h.view();
    assert.deepEqual(await h.labels(), ['Wrong password', 'Back']);
    assert.equal(v.sel, 1, 'lands on Back');
    await h.push();
    assert.deepEqual(await h.labels(), ['Connect', 'Forget...']);
  });
  await check('connect result after the user backed out toasts the outcome', async () => {
    await h.choose('Connect');
    assert.deepEqual(await h.labels(), ['Connecting to Office...']);
    await h.back();
    assert.deepEqual(await h.labels(), ['Connect', 'Forget...']);
    await h.bridge(fixture('controller/event.wifi_connect_result', { ssid: 'Office' }));
    assert.equal((await h.view()).toast, 'Connected to Office');
  });
  await check('scan: sends wifi_scan, shows Scanning..., then results with security and bars', async () => {
    await h.back(); await h.back(); // Saved networks -> Wi-Fi
    assert.equal((await h.view()).title, 'Wi-Fi');
    const mark = await h.mark();
    await h.choose('Scan for networks...');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_scan' }]);
    let v = await h.view();
    assert.equal(v.title, 'Wi-Fi networks');
    // One "Scanning..." only (it used to appear twice: a label plus the Rescan row).
    assert.deepEqual(await h.labels(), ['Scanning...']);
    await h.bridge(fixture('controller/event.wifi_scan_started'));
    assert.deepEqual(await h.labels(), ['Scanning...']);
    await h.bridge(fixture('controller/event.wifi_scan_results', {
      networks: [
        { ssid: 'Cafe', signal_pct: 60, signal_dbm: -60, security: 'wpa2', supported: true },
        { ssid: 'OpenNet', signal_pct: 30, signal_dbm: -80, security: 'open', supported: true },
        { ssid: 'Legacy', signal_pct: 20, signal_dbm: -85, security: 'wep', supported: false },
        { ssid: 'Home', signal_pct: 90, signal_dbm: -50, security: 'wpa2', supported: true },
      ],
    }));
    v = await h.view();
    assert.deepEqual(await h.labels(), ['Cafe|WPA2', 'OpenNet|Open', 'Legacy|WEP', 'Home|WPA2 (saved)', 'Rescan']);
    assert.deepEqual(v.rows.map((r) => r.bars), [3, 2, 1, 4, null]);
  });
  await check('unsupported network toasts and does not navigate', async () => {
    await h.choose('Legacy');
    const v = await h.view();
    assert.equal(v.title, 'Wi-Fi networks');
    assert.equal(v.toast, 'WEP networks are not supported here');
  });
  await check('open network connects without a password (empty password in the message)', async () => {
    const mark = await h.mark();
    await h.choose('OpenNet');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_connect', ssid: 'OpenNet', password: '' }]);
    assert.equal((await h.view()).title, 'OpenNet');
    await h.bridge(fixture('controller/event.wifi_connect_result.failed', { ssid: 'OpenNet', message: 'Timed out' }));
    assert.deepEqual(await h.labels(), ['Timed out', 'Back']);
    await h.push();
    assert.equal((await h.view()).title, 'Wi-Fi networks');
  });
  await check('a saved network in the scan list connects with stored credentials', async () => {
    const mark = await h.mark();
    await h.choose('Home');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_connect_saved', ssid: 'Home' }]);
    await h.back();
  });
  await check('Rescan sends wifi_scan; a second Rescan while scanning is a no-op', async () => {
    const mark = await h.mark();
    await h.choose('Rescan');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_scan' }]);
    await h.bridge(fixture('controller/event.wifi_scan_started'));
    const m2 = await h.mark();
    await h.choose('Scanning...');
    assert.deepEqual(await h.sentSince(m2), []);
  });
  await check('wifi_error toasts and clears the scanning state', async () => {
    await h.bridge(fixture('controller/event.wifi_error'));
    const v = await h.view();
    assert.equal(v.toast, 'Wi-Fi error (wifi_scan): no wireless interface');
    assert.equal(v.rows[v.rows.length - 1].label, 'Rescan');
  });
  await check('adapter unplugged pops every Wi-Fi screen and hides the row', async () => {
    await h.bridge(fixture('controller/event.wifi_available.none'));
    const v = await h.view();
    assert.equal(v.title, 'Settings');
    assert.deepEqual(v.rows.map((r) => r.label.split(':')[0]), ['Bluetooth', 'Brightness', 'Terminal', 'Timezone', 'Users']);
  });
});

// =============================================================================
const NETS = [
  { ssid: 'Cafe', signal_pct: 60, signal_dbm: -60, security: 'wpa2', supported: true },
  { ssid: 'OpenNet', signal_pct: 30, signal_dbm: -80, security: 'open', supported: true },
];
async function gotoPasswordScreen(h) {
  await h.bridge(fixture('controller/event.wifi_available'));
  await h.bridge(fixture('controller/event.wifi_status', { state: 'disconnected', ssid: null, ip: null, signal_pct: null }));
  await h.openMenu();
  await h.choose('Wi-Fi');
  await h.choose('Scan for networks...');
  await h.bridge(fixture('controller/event.wifi_scan_results', { networks: NETS }));
  await h.choose('Cafe');
}

await scenario('Wi-Fi password: keyboard entry, knob picker, connect', async (h, page, srv) => {
  await gotoPasswordScreen(h);
  await check('secured network opens the password screen', async () => {
    const v = await h.view();
    assert.equal(v.title, 'Cafe');
    assert.deepEqual(await h.labels(), ['Password|(empty)', 'Show password|OFF', 'Pick characters with knob...', 'Connect']);
    assert.equal(v.hint, 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back (discards password)');
    assert.equal(v.inputShown, false);
  });
  await check('Connect with a too-short password toasts and sends nothing', async () => {
    const mark = await h.mark();
    await h.choose('Connect');
    assert.deepEqual(await h.sentSince(mark), []);
    assert.equal((await h.view()).toast, 'Password must be 8 to 63 characters');
  });
  await check('selecting the Password row starts text entry in the real input', async () => {
    await h.moveTo('Password');
    await h.push();
    const v = await h.view();
    assert.equal(v.inputShown, true);
    assert.equal(v.inputFocused, true);
    assert.equal(v.hint, 'type the password \u00b7 enter or esc: done');
  });
  await check('knob does nothing while typing', async () => {
    await h.knob(1);
    assert.equal((await h.view()).sel, 0);
  });
  await check('typed characters are mirrored masked into the row', async () => {
    await page.keyboard.type('hunter2hunter2');
    const v = await h.view();
    assert.equal(v.rows[0].value, '\u2022'.repeat(14));
    assert.equal(v.inputValue, 'hunter2hunter2');
  });
  await check('typing into the field does not reach the PTY', async () => {
    assert.deepEqual((await h.sentSince(0, AGENT)).filter((m) => m.type === 'input'), []);
  });
  await check('Enter in the field ends editing and returns focus to the screen', async () => {
    await page.keyboard.press('Enter');
    const v = await h.view();
    assert.equal(v.inputShown, false);
    assert.equal(v.inputFocused, false);
    assert.equal(v.hint, 'knob: move \u00b7 esc: edit / select \u00b7 esc (hold): back (discards password)');
    assert.equal(v.rows[0].value, '\u2022'.repeat(14));
  });
  await sleep(500);
  await check('Show password toggle reveals the text', async () => {
    await h.moveTo('Show password');
    await h.push();
    const v = await h.view();
    assert.equal(v.rows[1].value, 'ON');
    assert.equal(v.rows[0].value, 'hunter2hunter2');
  });
  await check('Connect sends wifi_connect with the password and shows the connecting screen', async () => {
    const mark = await h.mark();
    await h.choose('Connect');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'wifi_connect', ssid: 'Cafe', password: 'hunter2hunter2' }]);
    assert.equal((await h.view()).title, 'Cafe');
    assert.deepEqual(await h.labels(), ['Connecting to Cafe...']);
  });
  await check('success result: Connected + Done highlighted; Done returns to the Wi-Fi home and discards the password', async () => {
    await h.bridge(fixture('controller/event.wifi_connecting', { ssid: 'Cafe' }));
    await h.bridge(fixture('controller/event.wifi_connect_result', { ssid: 'Cafe' }));
    const v = await h.view();
    assert.deepEqual(await h.labels(), ['Connected', 'Done']);
    assert.equal(v.sel, 1);
    await h.push();
    assert.equal((await h.view()).title, 'Wi-Fi');
    assert.equal((await h.view()).inputValue, '');
  });
});

await scenario('Wi-Fi password: knob picker', async (h, page, srv) => {
  await gotoPasswordScreen(h);
  await h.choose('Pick characters');
  const picker = () => page.evaluate(() => ({
    pw: document.querySelector('.wifi-picker-pw').textContent,
    count: document.querySelector('.wifi-picker-count').textContent,
    cells: Array.from(document.querySelectorAll('.wifi-picker-cell')).map((c) => c.textContent + (c.classList.contains('cur') ? '*' : '')),
  }));
  await check('picker screen shows the ring around the start character', async () => {
    const v = await h.view();
    assert.equal(v.title, 'Pick characters');
    assert.equal(v.hint, 'knob: choose \u00b7 esc: add / apply \u00b7 esc (hold): back');
    const p = await picker();
    assert.equal(p.pw, '(empty)_');
    assert.equal(p.count, '0 / 63 characters');
    assert.deepEqual(p.cells, ['~', '[OK]', '[DEL]', 'a*', 'b', 'c', 'd']);
  });
  await check('knob moves the ring; push appends the character (masked)', async () => {
    await h.knob(1);
    assert.equal((await picker()).cells[3], 'b*');
    await h.push();
    const p = await picker();
    assert.equal(p.pw, '\u2022_');
    assert.equal(p.count, '1 / 63 characters');
  });
  await check('[DEL] removes the last character; [OK] returns to the password screen on Connect', async () => {
    await h.knob(-1); // a
    await h.knob(-1); // [DEL]
    assert.equal((await picker()).cells[3], '[DEL]*');
    await h.push();
    assert.equal((await picker()).pw, '(empty)_');
    await h.knob(-1); // [OK]
    assert.equal((await picker()).cells[3], '[OK]*');
    await h.push();
    const v = await h.view();
    assert.equal(v.title, 'Cafe');
    assert.equal(v.sel, 3);
  });
  await check('touch: tapping a neighbour cell moves there; tapping the centre applies it', async () => {
    await h.push(); // Connect with empty password: toast, stays
    await h.moveTo('Pick characters');
    await h.push();
    await page.evaluate(() => document.querySelectorAll('.wifi-picker-cell')[4].click()); // 'b'
    assert.equal((await picker()).cells[3], 'b*');
    await page.evaluate(() => document.querySelectorAll('.wifi-picker-cell')[3].click());
    assert.equal((await picker()).pw, '\u2022_');
  });
  await check('leaving the password screen with ESC hold discards the typed text', async () => {
    await h.escHold(); // back from picker
    assert.equal((await h.view()).title, 'Cafe');
    assert.equal((await h.view()).rows[0].value, '\u2022');
    await h.escHold(); // back from password screen
    assert.equal((await h.view()).title, 'Wi-Fi networks');
    await h.choose('Cafe');
    assert.equal((await h.view()).rows[0].value, '(empty)');
  });
});

// =============================================================================
await scenario('knob-push double delivery guard (400 ms)', async (h, page, srv) => {
  await gotoPasswordScreen(h);
  await h.moveTo('Password');
  await check('order A: DOM Enter ends editing, the bridge button 28 that follows within 400 ms is swallowed', async () => {
    await h.push();
    assert.equal((await h.view()).inputShown, true);
    await page.keyboard.press('Enter');
    assert.equal((await h.view()).inputShown, false);
    await h.btn(28, 1);
    await sleep(60);
    let v = await h.view();
    assert.equal(v.inputShown, false, 'swallowed press must not reopen the field');
    assert.equal(v.inputFocused, false);
    await sleep(450);
    await h.btn(28, 1);
    await sleep(60);
    v = await h.view();
    assert.equal(v.inputShown, true, 'a press after the window opens the field again');
    await page.keyboard.press('Enter');
    await sleep(450);
  });
  await check('order B: bridge button 28 ends editing, the DOM Enter that follows is swallowed (no PTY input, no re-open)', async () => {
    await h.btn(28, 1);
    await sleep(500);
    assert.equal((await h.view()).inputShown, true);
    const mark = await h.mark(AGENT);
    await h.btn(28, 1);
    assert.equal((await h.view()).inputShown, false);
    await page.keyboard.press('Enter');
    await h.btn(28, 1);
    await sleep(60);
    const v = await h.view();
    assert.equal(v.inputShown, false);
    assert.deepEqual(await h.sentSince(mark, AGENT), []);
  });
  await check('knob moves and KEY_1 back still work inside the guard window', async () => {
    await sleep(450);
    await h.knob(1);
    assert.equal((await h.view()).sel, 1);
  });
  await check('ESC tap also ends editing and is covered by the same guard', async () => {
    await h.moveTo('Password');
    await h.push();
    assert.equal((await h.view()).inputShown, true);
    await h.escTap();
    assert.equal((await h.view()).inputShown, false);
    await h.escTap();
    assert.equal((await h.view()).inputShown, false);
  });
  await check('ESC hold while typing ends editing and keeps the screen', async () => {
    await sleep(450);
    await h.push();
    assert.equal((await h.view()).inputShown, true);
    await h.escHold();
    const v = await h.view();
    assert.equal(v.inputShown, false);
    assert.equal(v.title, 'Cafe');
  });
  await check('blur of the field (on-screen keyboard dismissed) ends editing', async () => {
    await sleep(450);
    await h.push();
    assert.equal((await h.view()).inputShown, true);
    await page.evaluate(() => document.getElementById('wifi-pass-input').blur());
    assert.equal((await h.view()).inputShown, false);
  });
});

// =============================================================================
await scenario('tab switch with buttons 1..4', async (h, page, srv) => {
  await h.agent(fixture('terminal/server_session_list', { sessions: [{ index: 0, name: 'glances' }, { index: 1, name: 'bash' }, { index: 2, name: 'logs' }] }));
  await h.agent(fixture('terminal/server_output', { session: 0, data: 'zero-output\r\n' }));
  await h.agent(fixture('terminal/server_output', { session: 1, data: 'one-output\r\n' }));
  await check('buttons 1..4 map to sessions 0..3, a missing session is a no-op', async () => {
    const mark = await h.mark(AGENT);
    await h.press(3); // KEY_2 -> session 1
    assert.deepEqual((await h.tabs()).map((t) => t.active), [false, true, false]);
    assert.equal((await h.statusText()).session, 'session: bash');
    assert.match(await h.text(), /one-output/);
    assert.doesNotMatch(await h.text(), /zero-output/);
    await h.press(5); // KEY_4 -> session 3 does not exist
    assert.deepEqual((await h.tabs()).map((t) => t.active), [false, true, false]);
    await h.press(2); // KEY_1 -> back to session 0, replays its buffer
    assert.deepEqual((await h.tabs()).map((t) => t.active), [true, false, false]);
    assert.match(await h.text(), /zero-output/);
    assert.doesNotMatch(await h.text(), /one-output/);
    const resizes = (await h.sentSince(mark, AGENT)).filter((m) => m.type === 'resize').map((m) => m.session);
    assert.deepEqual(resizes, [1, 0]);
  });
  await check('re-pressing the active tab sends nothing; a dead session gets a respawn control message', async () => {
    const mark = await h.mark(AGENT);
    await h.press(2);
    assert.deepEqual(await h.sentSince(mark, AGENT), []);
    await h.agent(fixture('terminal/server_exited_needs_manual_respawn', { session: 2 }));
    await h.press(4); // session 2: dead, so switching to it also sends respawn
    assert.match(await h.text(), /session exited -- select this tab again to restart it/);
    assert.deepEqual((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'control'), [{ type: 'control', session: 2, action: 'respawn' }]);
    await h.press(2); await h.press(4); // away and back: the dead flag was cleared
    assert.equal((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'control').length, 1);
  });
  await check('exited with auto respawn prints a restart notice into the session', async () => {
    await h.press(2);
    await h.agent(fixture('terminal/server_exited_auto_respawning', { session: 0, code: 3 }));
    assert.match(await h.text(), /session exited, code=3 -- restarting\.\.\./);
  });
});

await scenario('dead session respawn on tab select', async (h, page, srv) => {
  await h.agent(fixture('terminal/server_session_list', { sessions: [{ index: 0, name: 'a' }, { index: 1, name: 'b' }] }));
  await h.agent(fixture('terminal/server_exited_needs_manual_respawn', { session: 1 }));
  await check('selecting a dead tab sends control respawn once and clears the dead flag', async () => {
    const mark = await h.mark(AGENT);
    await h.press(3);
    assert.deepEqual((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'control'), [{ type: 'control', session: 1, action: 'respawn' }]);
    await h.press(2); await h.press(3);
    assert.equal((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'control').length, 1);
  });
  await check('buttons are ignored while the menu is open (no tab change)', async () => {
    await h.press(2);
    await h.openMenu();
    await h.press(3);
    assert.deepEqual((await h.tabs()).map((t) => t.active), [true, false]);
  });
});

// =============================================================================
await scenario('reconnect after the socket closes', async (h, page, srv) => {
  await check('bridge drop: status flips to DISCONNECTED, reconnects after ~2 s and re-requests settings', async () => {
    const opensBefore = await srv.opens(BRIDGE);
    await srv.drop(BRIDGE);
    await sleep(50);
    let s = await h.statusText();
    assert.equal(s.bridge, 'BRIDGE: DISCONNECTED');
    assert.equal(s.bridgeOk, false);
    await sleep(1400);
    assert.equal(await srv.opens(BRIDGE), opensBefore, 'must not have reconnected before ~2 s');
    const mark = await h.mark();
    await page.waitForFunction(() => document.getElementById('bridge-text').textContent === 'BRIDGE: LIVE', null, { timeout: 2500 });
    assert.equal(await srv.opens(BRIDGE), opensBefore + 1);
    await sleep(50);
    assert.deepEqual(await h.sentSince(mark), [{ type: 'bt_power_get' }, { type: 'brightness_get' }, { type: 'timezone_get' }, { type: 'wifi_get' }, { type: 'user_bootstrap_get' }]);
  });
  await check('agent drop: reconnects after ~2 s and re-sends a resize', async () => {
    const opensBefore = await srv.opens(AGENT);
    await srv.drop(AGENT);
    await sleep(50);
    assert.equal((await h.statusText()).agent, 'AGENT: DISCONNECTED');
    const mark = await h.mark(AGENT);
    await page.waitForFunction(() => document.getElementById('agent-text').textContent === 'AGENT: LIVE', null, { timeout: 3500 });
    assert.equal(await srv.opens(AGENT), opensBefore + 1);
    await sleep(50);
    assert.equal((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'resize').length >= 1, true);
  });
  await check('refused connections keep retrying until the server is back', async () => {
    await srv.refuse(BRIDGE, true);
    await srv.drop(BRIDGE);
    await sleep(2600);
    assert.notEqual((await h.statusText()).bridge, 'BRIDGE: LIVE');
    await srv.refuse(BRIDGE, false);
    await page.waitForFunction(() => document.getElementById('bridge-text').textContent === 'BRIDGE: LIVE', null, { timeout: 3500 });
  });
});

// =============================================================================
await scenario('clock sync', async (h, page, srv) => {
  const clock = () => page.evaluate(() => ({
    text: document.getElementById('clock-text').textContent,
    ok: document.getElementById('clock-dot').classList.contains('ok'),
    stale: document.getElementById('clock-dot').classList.contains('stale'),
    hour: new Date().getHours(),
  }));
  await check('before any sync the dot is stale and not ok', async () => {
    await sleep(1100);
    const c = await clock();
    assert.equal(c.ok, false);
    assert.match(c.text, /^\d\d:\d\d:\d\d$/);
  });
  await check('clock_sync ok applies the offset and lights the dot', async () => {
    await h.agent(fixture('terminal/server_clock_sync_ok', { offset_ms: 3600000 }));
    const c = await clock();
    assert.equal(c.ok, true);
    assert.equal(c.stale, false);
    assert.equal(Number(c.text.slice(0, 2)), (c.hour + 1) % 24);
  });
  await check('stale and unsynced statuses mark the dot stale', async () => {
    await h.agent(fixture('terminal/server_clock_sync_stale'));
    let c = await clock();
    assert.equal(c.ok, false);
    assert.equal(c.stale, true);
    await h.agent(fixture('terminal/server_clock_sync_unsynced'));
    c = await clock();
    assert.equal(c.ok, false);
    assert.equal(c.stale, true);
  });
  await check('the clock ticks once per second', async () => {
    await h.agent(fixture('terminal/server_clock_sync_ok', { offset_ms: 0 }));
    const a = (await clock()).text;
    await sleep(2200);
    const b = (await clock()).text;
    assert.notEqual(a, b);
  });
});

await scenario('terminal fits above the status bar', async (h, page, srv) => {
  const measure = () => page.evaluate(() => {
    const r = (sel) => { const b = document.querySelector(sel).getBoundingClientRect(); return { top: b.top, bottom: b.bottom, height: b.height }; };
    const rows = document.querySelector('.xterm-rows');
    const lastRow = rows.lastElementChild.getBoundingClientRect();
    return {
      tabBar: r('.tab-bar'), wrap: r('.screen-wrap'), screen: r('.xterm-screen'), status: r('.status-bar'),
      lastRowBottom: lastRow.bottom, rowsBottom: rows.getBoundingClientRect().bottom,
      rowCount: rows.children.length, innerHeight: window.innerHeight, scrollHeight: document.documentElement.scrollHeight,
    };
  });
  const fill = async () => {
    await h.agent(fixture('terminal/server_output', { session: 0, data: Array.from({ length: 120 }, (_, i) => 'line' + i).join('\r\n') + '\r\n$ ' }));
    await sleep(250);
  };
  const assertFits = (m) => {
    const ctx = JSON.stringify(m);
    assert.ok(m.screen.bottom <= m.status.top + 0.5, 'xterm-screen bottom ' + m.screen.bottom + ' overlaps status top ' + m.status.top + ' ' + ctx);
    assert.ok(m.lastRowBottom <= m.status.top + 0.5, 'last row bottom ' + m.lastRowBottom + ' overlaps status top ' + m.status.top);
    assert.ok(m.rowsBottom <= m.wrap.bottom + 0.5, 'rows spill out of the wrapper ' + ctx);
    assert.ok(m.status.bottom <= m.innerHeight + 0.5, 'status bar pushed off the viewport ' + ctx);
    assert.ok(m.scrollHeight <= m.innerHeight, 'document scrolls ' + ctx);
  };
  await h.agent(fixture('terminal/server_session_list'));
  await fill();
  await check('prompt on the last row sits above the status bar (tabs rendered)', async () => assertFits(await measure()));
  await check('the prompt row is the last visible row and shows the cursor line', async () => {
    const t = await page.evaluate(() => {
      const rows = document.querySelector('.xterm-rows').children;
      return { last: rows[rows.length - 1].textContent, cursorRow: document.querySelectorAll('.xterm-rows > div').length };
    });
    assert.match(t.last, /\$/);
  });
  await check('tab bar growth triggers a refit (fewer rows, still above the status bar)', async () => {
    const before = await measure();
    await page.evaluate(() => { document.getElementById('tab-bar').style.padding = '30px 8px'; });
    await sleep(300);
    await fill();
    const after = await measure();
    assert.ok(after.rowCount < before.rowCount, 'rows ' + before.rowCount + ' -> ' + after.rowCount);
    assertFits(after);
  });
  await check('a window resize refits above the status bar', async () => {
    await page.evaluate(() => { document.getElementById('tab-bar').style.padding = ''; });
    await page.setViewportSize({ width: 800, height: 400 });
    await sleep(300);
    await fill();
    assertFits(await measure());
    await page.setViewportSize({ width: 800, height: 480 });
    await sleep(300);
    await fill();
    assertFits(await measure());
  });
});

// =============================================================================
// First-boot overlay (delta plan sections 5.3 and 6). The controller owns the
// phases and counts the physical M presses; the mock plays it by sending
// user_bootstrap and user_bootstrap_secret events.
const FB_SECRET = fixture('controller/event.user_bootstrap_secret').password;
const FB_GROUPS = ['Xe4m', 'H7pQ', '9tLw', 'Zc3K'];
const fbPhase = (p) => fixture('controller/event.user_bootstrap.' + p);

const fbState = (page) => page.evaluate(() => {
  const el = document.getElementById('first-boot');
  const r = el.getBoundingClientRect();
  const top = document.elementFromPoint(400, 240);
  return {
    cls: el.className, text: el.textContent, rect: [r.left, r.top, r.width, r.height],
    topIsOverlay: !!top && el.contains(top),
    inputs: Array.from(el.querySelectorAll('input')).length,
    sel: getComputedStyle(el.querySelector('.fb-secret') || el).userSelect || getComputedStyle(el).webkitUserSelect,
  };
});
const secretLeaks = (page) => page.evaluate((groups) => {
  const t = document.body.textContent;
  return groups.some((g) => t.indexOf(g) !== -1) || Object.keys(localStorage).length > 0 || Object.keys(sessionStorage).length > 0;
}, FB_GROUPS);

await scenario('first boot: prompt, generating, showing, done', async (h, page, srv) => {
  await check('no overlay before any event', async () => {
    assert.match((await fbState(page)).cls, /hidden/);
  });
  await check('prompt: full-screen message, arrow, above the menu', async () => {
    await h.bridge(fbPhase('prompt'));
    await sleep(60);
    const f = await fbState(page);
    assert.match(f.cls, /blocking/);
    assert.deepEqual(f.rect, [0, 0, 800, 480]);
    assert.equal(f.topIsOverlay, true);
    assert.match(f.text, /Kiosk password not set/);
    assert.match(f.text, /kiosk user password is unset/);
    assert.match(f.text, /M button/);
    assert.equal(await page.evaluate(() => !!document.querySelector('#first-boot .fb-arrow')), true);
  });
  await check('generating then showing: the password replaces the message, no timer, no toast', async () => {
    await h.bridge(fbPhase('generating'));
    await sleep(40);
    assert.match((await fbState(page)).text, /Generating/);
    await h.bridge(fbPhase('showing'));
    await h.bridge(fixture('controller/event.user_bootstrap_secret'));
    await sleep(60);
    const f = await fbState(page);
    assert.doesNotMatch(f.text, /Kiosk password not set/);
    assert.deepEqual(await page.evaluate(() => Array.from(document.querySelectorAll('#first-boot .fb-secret-group')).map((g) => g.textContent)), FB_GROUPS);
    assert.match(f.text, /Press M again/);
    assert.equal(f.inputs, 0);
    assert.equal(f.sel, 'none');
    const toast = await page.evaluate(() => document.getElementById('menu-toast').classList.contains('hidden'));
    assert.equal(toast, true, 'the password is not a toast');
    await sleep(1200); // no timeout of any kind
    assert.match((await fbState(page)).cls, /blocking/);
    assert.equal(await page.evaluate(() => document.getElementById('first-boot').textContent.indexOf('Xe4m') !== -1), true);
  });
  await check('the log and the store never contain the password', async () => {
    const dump = await page.evaluate(() => JSON.stringify(TR.logging.dump()) + JSON.stringify(TR.boot.app.store.getState()));
    assert.equal(dump.indexOf(FB_SECRET), -1);
  });
  await check('a Wi-Fi toast arriving during showing renders below the overlay and cannot replace the password', async () => {
    await h.bridge(fixture('controller/event.wifi_error', { context: 'wifi_connect', message: 'boom' }));
    await sleep(60);
    const f = await fbState(page);
    assert.equal(f.topIsOverlay, true);
    assert.equal(await page.evaluate(() => document.querySelectorAll('#first-boot .fb-secret-group').length), 4);
  });
  await check('done: overlay gone, password in no DOM and no storage, nothing sent by the frontend', async () => {
    const mark = await h.mark();
    await h.bridge(fbPhase('done'));
    await sleep(60);
    assert.match((await fbState(page)).cls, /hidden/);
    assert.equal(await secretLeaks(page), false);
    assert.deepEqual(await h.sentSince(mark), [], 'the second M press is the controller\'s, the frontend sends no ack');
    const dump = await page.evaluate(() => JSON.stringify(TR.logging.dump()));
    assert.equal(dump.indexOf(FB_SECRET), -1);
  });
});

await scenario('first boot: every input path is dead while the overlay is up', async (h, page, srv) => {
  const sentAgent = async (mark) => (await h.sentSince(mark, AGENT)).filter((m) => m.type === 'input');
  await h.agent(fixture('terminal/server_session_list', { sessions: [{ index: 0, name: 'a' }, { index: 1, name: 'b' }] }));
  await h.bridge(fbPhase('prompt'));
  await sleep(60);
  await check('DOM keydowns, including raw m, Escape and Enter, reach nothing', async () => {
    const mark = await h.mark(AGENT);
    for (const k of ['m', 'a', 'Enter', 'Escape', 'ArrowUp', 'Control+c', 'Backspace']) await page.keyboard.press(k);
    await page.keyboard.type('hunter2');
    assert.deepEqual(await sentAgent(mark), []);
    assert.equal((await h.view()).open, false);
  });
  await check('the sendInput sink drops bytes from any path', async () => {
    const mark = await h.mark(AGENT);
    await page.evaluate(() => document.getElementById('screen').dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true })));
    assert.deepEqual(await sentAgent(mark), []);
  });
  await check('ESC, buttons 1-4, knob and knob push do nothing', async () => {
    const mark = await h.mark(AGENT);
    await h.press(1); await h.escHold();
    for (const c of [2, 3, 4, 5]) await h.press(c);
    await h.knob(1); await h.knob(-1); await h.push();
    await h.btn(50, 1); await h.btn(50, 0); // M is the controller's business
    assert.equal((await h.view()).open, false);
    assert.deepEqual((await h.sentSince(mark, AGENT)).filter((m) => m.type === 'control'), []);
    assert.deepEqual((await h.tabs()).map((t) => t.active), [true, false]);
  });
  await check('touch and click on the overlay do not refocus the terminal or reach the menu', async () => {
    await page.touchscreen.tap(400, 240).catch(() => page.mouse.click(400, 240));
    await page.mouse.click(400, 240);
    assert.equal((await h.view()).open, false);
    assert.equal(await page.evaluate(() => document.getElementById('wifi-pass-input').disabled), true);
    assert.equal(await page.evaluate(() => document.activeElement && document.activeElement.id === 'wifi-pass-input'), false);
  });
  await check('the same holds in showing', async () => {
    await h.bridge(fbPhase('showing'));
    await h.bridge(fixture('controller/event.user_bootstrap_secret'));
    const mark = await h.mark(AGENT);
    await page.keyboard.press('m'); await page.keyboard.press('Escape'); await h.press(1); await h.push();
    assert.deepEqual(await sentAgent(mark), []);
    assert.equal((await h.view()).open, false);
    assert.equal(await page.evaluate(() => document.querySelectorAll('#first-boot .fb-secret-group').length), 4);
  });
  await check('after done the keys, ESC and buttons work again', async () => {
    await h.bridge(fbPhase('done'));
    await sleep(60);
    const mark = await h.mark(AGENT);
    await page.keyboard.press('a');
    assert.deepEqual((await sentAgent(mark)).map((m) => m.data), ['a']);
    await h.esc();
    assert.equal((await h.view()).open, true);
  });
});

await scenario('first boot: wipes on connection close, reload, and re-shows on reconnect', async (h, page, srv) => {
  await check('a closed bridge wipes the password and the screen; reconnect asks again', async () => {
    await h.bridge(fbPhase('showing'));
    await h.bridge(fixture('controller/event.user_bootstrap_secret'));
    await sleep(60);
    assert.equal(await secretLeaks(page), true);
    await srv.drop(BRIDGE);
    await sleep(120);
    assert.equal(await secretLeaks(page), false);
    assert.match((await fbState(page)).cls, /hidden/);
    await page.waitForFunction(() => window.__mock.live('ws://127.0.0.1:8765'), null, { timeout: 8000 });
    await sleep(200);
    const sent = await page.evaluate(() => window.__mock.sent.map((x) => x.msg.type));
    assert.ok(sent.filter((t) => t === 'user_bootstrap_get').length >= 2, 'get sent again: ' + sent.join(','));
    await h.bridge(fbPhase('showing'));
    await h.bridge(fixture('controller/event.user_bootstrap_secret'));
    await sleep(60);
    assert.equal(await page.evaluate(() => document.querySelectorAll('#first-boot .fb-secret-group').length), 4);
  });
  await check('a phase that is not showing wipes it', async () => {
    await h.bridge(fbPhase('prompt'));
    await sleep(60);
    assert.equal(await secretLeaks(page), false);
  });
  await check('a reload starts empty and asks again', async () => {
    await page.reload();
    await page.waitForFunction(() => document.getElementById('bridge-text').textContent === 'BRIDGE: LIVE', null, { timeout: 5000 });
    assert.equal(await secretLeaks(page), false);
    assert.match((await fbState(page)).cls, /hidden/);
    assert.equal(await page.evaluate(() => window.__mock.sent.map((x) => x.msg.type).indexOf('user_bootstrap_get') !== -1), true);
  });
});

await scenario('first boot: storage failure shows a note and blocks nothing', async (h, page, srv) => {
  await check('storage_ok false: strip visible, ESC still opens the menu', async () => {
    await h.bridge(fixture('controller/event.user_bootstrap.storage_failed'));
    await sleep(60);
    const f = await fbState(page);
    assert.match(f.cls, /note/);
    assert.match(f.text, /storage is not available/);
    await h.esc();
    assert.equal((await h.view()).open, true);
  });
});

// =============================================================================
// Account menu (T-P2.6): unlock gate, user list, add user, one-time password,
// hold-to-confirm, keys. The mock plays the controller by answering with the
// golden fixtures.
const UF = (name, over) => fixture('controller/' + name, over);
const SECRET_PW = UF('event.user_secret').password;
const typeAndEnd = async (h, page, text) => {
  await page.keyboard.type(text);
  await page.keyboard.press('Enter');
  await sleep(450); // enter_guard_ms
};
const typeField = async (h, page, label, text) => {
  await h.choose(label);
  assert.equal((await h.view()).inputFocused, true, 'input focused for ' + label);
  await typeAndEnd(h, page, text);
};
const unlockViaMenu = async (h, page) => {
  await h.openMenu();
  await h.bridge(UF('event.user_session', { username: 'kiosk' }));
  await h.bridge(UF('event.user_list'));
  await h.bridge(UF('event.group_list'));
  await h.choose('Users');
};
const storedAnywhere = (page, needle) => page.evaluate((n) => {
  const dump = JSON.stringify(TR.logging.dump()) + JSON.stringify(TR.boot.app.store.getState()) + JSON.stringify(TR.boot.app.getState().ui);
  return dump.indexOf(n) !== -1 || document.body.textContent.indexOf(n) !== -1 ||
    JSON.stringify(localStorage).indexOf(n) !== -1 || JSON.stringify(sessionStorage).indexOf(n) !== -1 ||
    Array.from(document.querySelectorAll('input')).some((i) => i.value.indexOf(n) !== -1);
}, needle);

await scenario('users: unlock with the admin password, wipe, lock on leave', async (h, page, srv) => {
  await h.openMenu();
  await check('root has a Users row; locked it opens the unlock gate without asking the controller anything', async () => {
    const mark = await h.mark();
    await h.choose('Users');
    const v = await h.view();
    assert.equal(v.title, 'Unlock accounts');
    assert.deepEqual(await h.labels(), ['Admin|kiosk', 'Password|(empty)', 'Show password|OFF', 'Unlock']);
    assert.deepEqual(await h.sentSince(mark), []);
  });
  await check('an empty password toasts and sends nothing', async () => {
    const mark = await h.mark();
    await h.choose('Unlock');
    assert.equal((await h.view()).toast, 'Enter the admin password');
    assert.deepEqual(await h.sentSince(mark), []);
  });
  await check('the password goes through the real masked input with the account limits, not the Wi-Fi ones', async () => {
    await h.choose('Password');
    const f = await page.evaluate(() => { const i = document.getElementById('wifi-pass-input'); return { type: i.type, max: i.maxLength, ph: i.placeholder, focused: document.activeElement === i }; });
    assert.deepEqual(f, { type: 'password', max: 128, ph: 'Admin password', focused: true });
    await typeAndEnd(h, page, 'correct horse 9');
    assert.equal((await h.view()).rows[1].value, '\u2022'.repeat(15));
  });
  await check('Unlock sends user_unlock once and the password is in no state, log, storage or DOM afterwards', async () => {
    const mark = await h.mark();
    await h.choose('Unlock');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_unlock', username: 'kiosk', password: 'correct horse 9' }]);
    assert.equal(await storedAnywhere(page, 'correct horse'), false);
    assert.equal((await h.view()).rows[1].value, '(empty)');
  });
  await check('a wrong password toasts the reason and stays on the gate', async () => {
    await h.bridge(UF('event.user_error', { context: 'user_unlock', code: 'bad_password', message: 'x' }));
    const v = await h.view();
    assert.equal(v.title, 'Unlock accounts');
    assert.equal(v.toast, 'Wrong password');
  });
  await check('user_session opens the list and the list is requested', async () => {
    const mark = await h.mark();
    await h.bridge(UF('event.user_session', { username: 'kiosk' }));
    assert.equal((await h.view()).title, 'Users');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_list_get' }, { type: 'group_list_get' }]);
    await h.bridge(UF('event.user_list'));
    assert.deepEqual((await h.labels()).slice(0, 2), ['kiosk|admin protected', 'ana|locked']);
  });
  await check('backing out of the section sends user_lock_session', async () => {
    const mark = await h.mark();
    await h.back();
    assert.equal((await h.view()).title, 'Settings');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_lock_session' }]);
  });
  await check('Users with a live session skips the gate', async () => {
    await h.bridge(UF('event.user_session', { username: 'kiosk' }));
    await h.choose('Users');
    assert.equal((await h.view()).title, 'Users');
  });
  await check('not_unlocked from the controller returns to the gate and drops the list', async () => {
    await h.bridge(UF('event.user_error', { context: 'user_list_get', code: 'not_unlocked', message: 'x' }));
    const v = await h.view();
    assert.equal(v.title, 'Unlock accounts');
    assert.equal(v.toast, 'Admin session ended. Unlock again');
    assert.equal(await page.evaluate(() => TR.boot.app.store.getState().users.list), null);
  });
});

await scenario('users: add user, one-time password shown once and cleared', async (h, page, srv) => {
  await unlockViaMenu(h, page);
  await h.choose('Add user...');
  await check('add screen defaults to a generated password', async () => {
    assert.deepEqual(await h.labels(), ['Name|(empty)', 'Administrator|OFF', 'Generate password|ON', 'Create user']);
  });
  await check('a taken or invalid name is refused before anything is sent', async () => {
    const mark = await h.mark();
    await typeField(h, page, 'Name', 'ana');
    await h.choose('Create user');
    assert.equal((await h.view()).toast, 'That name is already in use');
    assert.deepEqual(await h.sentSince(mark), []);
  });
  await check('Create sends user_add with an empty password (generate)', async () => {
    await h.choose('Name');
    await page.keyboard.press('Control+a');
    await typeAndEnd(h, page, 'bob');
    const mark = await h.mark();
    await h.choose('Create user');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_add', name: 'bob', password: '', admin: false }]);
  });
  await check('the result returns to the list; user_secret shows the grouped password large, not an input, not selectable', async () => {
    await h.bridge(UF('event.user_result', { op: 'user_add', name: 'bob' }));
    assert.equal((await h.view()).title, 'Users');
    await h.bridge(UF('event.user_secret', { name: 'bob' }));
    const v = await h.view();
    assert.equal(v.title, 'Password for bob');
    assert.deepEqual(v.rows.map((r) => r.label), ['Shown once. It cannot be shown again.', 'Xe4m H7pQ 9tLw Zc3K', 'Done']);
    assert.equal(v.sel, 2);
    const s = await page.evaluate(() => {
      const el = document.querySelector('#menu-list .secret-row');
      const cs = getComputedStyle(el);
      return { size: parseFloat(cs.fontSize), sel: cs.userSelect || cs.webkitUserSelect, inputs: document.querySelectorAll('#menu-list input').length, visible: el.getBoundingClientRect().width > 0 && el.scrollWidth <= el.parentElement.clientWidth + 1 };
    });
    assert.ok(s.size >= 20, 'large: ' + s.size);
    assert.equal(s.sel, 'none');
    assert.equal(s.inputs, 0);
    assert.equal(s.visible, true);
  });
  await check('no timer and no toast: it stays until Done', async () => {
    await sleep(1200);
    assert.equal((await h.view()).title, 'Password for bob');
  });
  await check('Done clears it from DOM, store and log and returns to the list', async () => {
    await h.choose('Done');
    assert.equal((await h.view()).title, 'Users');
    assert.equal(await storedAnywhere(page, SECRET_PW), false);
    assert.equal(await storedAnywhere(page, 'Xe4m'), false);
  });
  await check('a second secret, cleared by a lock reply instead', async () => {
    await h.bridge(UF('event.user_secret', { name: 'bob' }));
    assert.equal(await storedAnywhere(page, SECRET_PW), true);
    await h.bridge(UF('event.user_session.locked'));
    const v = await h.view();
    assert.equal(v.title, 'Unlock accounts');
    assert.equal(await storedAnywhere(page, SECRET_PW), false);
  });
  await check('a third, cleared by leaving with back and by closing the menu', async () => {
    await h.bridge(UF('event.user_session', { username: 'kiosk' }));
    await h.bridge(UF('event.user_list'));
    await h.bridge(UF('event.user_secret', { name: 'bob' }));
    assert.equal(await storedAnywhere(page, SECRET_PW), true);
    await h.back();
    assert.equal(await storedAnywhere(page, SECRET_PW), false);
    await h.back(); // leaves the section
    await h.bridge(UF('event.user_session', { username: 'kiosk' }));
    await h.choose('Users');
    await h.bridge(UF('event.user_secret', { name: 'bob' }));
    await h.escHold(); await h.escHold(); await h.escHold();
    assert.equal((await h.view()).open, false);
    assert.equal(await storedAnywhere(page, SECRET_PW), false);
  });
  await check('the secret does not survive a bridge drop and reconnect', async () => {
    await h.openMenu();
    await h.bridge(UF('event.user_session', { username: 'kiosk' }));
    await h.bridge(UF('event.user_list'));
    await h.choose('Users');
    await h.bridge(UF('event.user_secret', { name: 'bob' }));
    await srv.drop(BRIDGE);
    await sleep(150);
    assert.equal(await storedAnywhere(page, SECRET_PW), false);
    assert.equal(await page.evaluate(() => TR.boot.app.store.getState().users.session.unlocked), false);
  });
});

await scenario('users: hold-to-confirm for destructive actions', async (h, page, srv) => {
  await unlockViaMenu(h, page);
  await h.choose('ana');
  await h.choose('Remove user...');
  await check('the confirm screen opens on Cancel with the name in the title', async () => {
    assert.equal((await h.view()).sel, 0);
    assert.equal((await h.labels())[0], 'Cancel');
    assert.equal((await h.view()).title, 'Remove ana?');
  });
  await check('a tap on the destructive row sends nothing and says how to confirm', async () => {
    await h.moveTo('Remove ana');
    const mark = await h.mark();
    await h.push();
    assert.deepEqual(await h.sentSince(mark), []);
    assert.equal((await h.view()).toast, 'Hold the knob push to confirm');
    await h.escTap();
    assert.deepEqual(await h.sentSince(mark), []);
  });
  await check('holding the knob push fires at the threshold while it is still held, once', async () => {
    const mark = await h.mark();
    await h.btn(28, 1);
    assert.deepEqual(await h.sentSince(mark), [], 'not at the press');
    await sleep(700);
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_remove', name: 'ana', keep_home: false, confirm: 'ana' }]);
    await h.btn(28, 2);
    await h.btn(28, 0);
    await sleep(60);
    assert.equal((await h.sentSince(mark)).length, 1, 'the release and autorepeat add nothing');
  });
  await check('the result toasts and returns to the list; a guard error stays on the confirm', async () => {
    await h.bridge(UF('event.user_result', { op: 'user_remove', name: 'ana' }));
    assert.equal((await h.view()).title, 'Users');
    assert.match((await h.view()).toast, /User ana removed/);
  });
  await check('the failure text of a guard is shown', async () => {
    await h.choose('kiosk');
    assert.ok((await h.labels()).includes('Remove: The kiosk account is protected'));
    await h.bridge(UF('event.user_result.failed', { op: 'user_remove', name: 'kiosk', error: 'last_admin' }));
    assert.equal((await h.view()).toast, 'This would remove the last admin');
  });
});

await scenario('users: groups with the sudo confirm, and SSH keys', async (h, page, srv) => {
  await unlockViaMenu(h, page);
  await h.choose('ana');
  await h.choose('Groups...');
  await check('plain group toggles at once, sudo needs the held confirm with the group name', async () => {
    const mark = await h.mark();
    await h.choose('team');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'user_group_add', name: 'ana', group: 'team', confirm: '' }]);
    await h.choose('sudo');
    assert.equal((await h.view()).title, 'Add to sudo?');
    await h.moveTo('Add ana to sudo');
    const m2 = await h.mark();
    await h.btn(28, 1); await sleep(700); await h.btn(28, 0);
    assert.deepEqual(await h.sentSince(m2), [{ type: 'user_group_add', name: 'ana', group: 'sudo', confirm: 'sudo' }]);
    await h.bridge(UF('event.user_result', { op: 'user_group_add', name: 'ana' }));
    assert.equal((await h.view()).title, 'Groups: ana');
  });
  await h.back();
  await h.choose('SSH keys...');
  await h.bridge(UF('event.ssh_key_list'));
  await check('key list shows type, comment and a short fingerprint', async () => {
    const v = await h.view();
    assert.equal(v.title, 'SSH keys: ana');
    assert.equal(v.rows[0].label, 'ssh-ed25519 ana@laptop');
    assert.match(v.rows[0].value, /^SHA256:/);
  });
  await check('a key line longer than the Wi-Fi limit goes through the input and is validated before sending', async () => {
    await h.choose('Add key...');
    await h.choose('Public key');
    const info = await page.evaluate(() => { const i = document.getElementById('wifi-pass-input'); return { type: i.type, max: i.maxLength }; });
    assert.deepEqual(info, { type: 'text', max: 8192 });
    const line = 'ssh-ed25519 ' + 'A'.repeat(200) + ' ana@laptop';
    await page.keyboard.insertText(line);
    await page.keyboard.press('Enter');
    await sleep(450);
    const mark = await h.mark();
    await h.choose('Add key');
    assert.deepEqual(await h.sentSince(mark), [{ type: 'ssh_key_add', name: 'ana', key: line }]);
  });
  await check('after the result the line is gone and the key list is shown again', async () => {
    await h.bridge(UF('event.user_result', { op: 'ssh_key_add', name: 'ana' }));
    assert.equal((await h.view()).title, 'SSH keys: ana');
    assert.equal(await storedAnywhere(page, 'AAAAAAAAAAAAAAAA'), false);
  });
  await check('removing a key is a held confirm carrying the fingerprint', async () => {
    const fp = UF('event.ssh_key_list').keys[0].fingerprint;
    await h.choose('ssh-ed25519');
    await h.choose('Remove key...');
    await h.moveTo('Remove key');
    const mark = await h.mark();
    await h.btn(28, 1); await sleep(700); await h.btn(28, 0);
    assert.deepEqual(await h.sentSince(mark), [{ type: 'ssh_key_remove', name: 'ana', fingerprint: fp, confirm: fp }]);
  });
});

await scenario('users: the first-boot overlay still blocks the menu while it is up', async (h, page, srv) => {
  await check('with the overlay up ESC does not open the menu, so Users is unreachable', async () => {
    await h.bridge(fbPhase('prompt'));
    await sleep(60);
    await h.press(1);
    assert.equal((await h.view()).open, false);
    await h.bridge(fbPhase('done'));
  });
});

await browser.close();
console.log('\ne2e menu regression: ' + passed + ' passed, ' + failed + ' failed');
if (failed) {
  for (const f of failures) console.log(' - ' + f);
  process.exit(1);
}
