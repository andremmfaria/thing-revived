// Harness for the headless regression run: resolves playwright and a
// Chromium binary from the environment (nothing is added to the repo),
// loads wire-message fixtures, and wraps the in-page WebSocket mock
// (mock-websocket.js) as a "server" the test can push frames from.
//
// No dependency is installed by this repo. Resolution order for the library:
//   $PLAYWRIGHT_MODULE (path to a playwright or playwright-core package dir),
//   node resolution from the cwd and from this file, `npm root -g`,
//   then ~/.local/share/mise/installs/npm-playwright-cli/*/ (if present).
// For the browser: $CHROME_PATH, then the newest chromium under
// $PLAYWRIGHT_BROWSERS_PATH or ~/.cache/ms-playwright, then a system chromium.
import { createRequire } from 'node:module';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
export const FRONTEND_DIR = path.resolve(here, '..', '..');
export const REPO_DIR = path.resolve(FRONTEND_DIR, '..');
export const PAGE_URL = pathToFileURL(path.join(FRONTEND_DIR, 'index.html')).href;
export const BRIDGE = 'ws://127.0.0.1:8765';
export const AGENT = 'ws://127.0.0.1:8766';

function tryRequire(spec, fromDir) {
  try {
    return createRequire(path.join(fromDir, 'x.js'))(spec);
  } catch (e) {
    return null;
  }
}

function globDirs(root, re) {
  try {
    return fs.readdirSync(root).filter((n) => re.test(n)).map((n) => path.join(root, n));
  } catch (e) {
    return [];
  }
}

export function resolvePlaywright() {
  const tried = [];
  const candidates = [];
  if (process.env.PLAYWRIGHT_MODULE) candidates.push(process.env.PLAYWRIGHT_MODULE);
  for (const name of ['playwright', 'playwright-core']) {
    for (const dir of [process.cwd(), here]) {
      const m = tryRequire(name, dir);
      if (m && m.chromium) return m;
      tried.push(name + ' from ' + dir);
    }
  }
  try {
    const g = execFileSync('npm', ['root', '-g'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    candidates.push(path.join(g, 'playwright'), path.join(g, 'playwright-core'));
  } catch (e) { /* npm absent */ }
  const mise = path.join(os.homedir(), '.local/share/mise/installs/npm-playwright-cli');
  for (const v of globDirs(mise, /./)) {
    const hidden = path.join(v, 'node_modules', '.mise');
    for (const d of globDirs(hidden, /^playwright(-core)?@/)) candidates.push(path.join(d, 'node_modules', d.includes('-core@') ? 'playwright-core' : 'playwright'));
  }
  for (const c of candidates) {
    const m = tryRequire(c, here);
    if (m && m.chromium) return m;
    tried.push(c);
  }
  return null;
}

export function resolveChrome() {
  if (process.env.CHROME_PATH && fs.existsSync(process.env.CHROME_PATH)) return process.env.CHROME_PATH;
  const root = process.env.PLAYWRIGHT_BROWSERS_PATH || path.join(os.homedir(), '.cache', 'ms-playwright');
  const newest = (re, rel) => {
    const dirs = globDirs(root, re).sort().reverse();
    for (const d of dirs) {
      const p = path.join(d, rel);
      if (fs.existsSync(p)) return p;
    }
    return null;
  };
  return (
    newest(/^chromium_headless_shell-/, 'chrome-headless-shell-linux64/chrome-headless-shell') ||
    newest(/^chromium-/, 'chrome-linux64/chrome') ||
    ['/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find((p) => fs.existsSync(p)) ||
    null
  );
}

// ---- fixtures ------------------------------------------------------------
// fixture('controller/event.bt_power', {powered:false}): the shared file under
// config/protocol/ (the single source of wire examples; no local copies), then
// Object.assign overrides. A missing file is an error, not a silent fallback.
export function fixture(key, overrides) {
  const shared = path.join(REPO_DIR, 'config', 'protocol', key + '.json');
  if (!fs.existsSync(shared)) throw new Error('no shared fixture config/protocol/' + key + '.json');
  return Object.assign(JSON.parse(fs.readFileSync(shared, 'utf8')), overrides || {});
}

// ---- server wrapper --------------------------------------------------------
export class MockServer {
  constructor(page) { this.page = page; }
  async push(url, msg) { await this.page.evaluate(([u, m]) => window.__mock.deliver(u, m), [url, msg]); }
  bridge(msg) { return this.push(BRIDGE, msg); }
  agent(msg) { return this.push(AGENT, msg); }
  async sent(url) {
    const all = await this.page.evaluate(() => window.__mock.sent);
    return all.filter((s) => !url || s.url === url);
  }
  // Messages sent after `mark` (a count taken earlier with sentCount()).
  async sentSince(mark, url) { return (await this.sent(url)).slice(mark).map((s) => s.msg); }
  async sentCount(url) { return (await this.sent(url)).length; }
  drop(url) { return this.page.evaluate((u) => window.__mock.drop(u), url); }
  opens(url) { return this.page.evaluate((u) => window.__mock.opens(u), url); }
  live(url) { return this.page.evaluate((u) => window.__mock.live(u), url); }
  refuse(url, on) { return this.page.evaluate(([u, o]) => window.__mock.refuse(u, o), [url, on]); }
}

export const MOCK_WS_SOURCE = fs.readFileSync(path.join(here, 'mock-websocket.js'), 'utf8');
