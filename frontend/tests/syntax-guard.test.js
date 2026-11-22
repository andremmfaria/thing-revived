'use strict';
// Fails if any shipped frontend source uses syntax or APIs newer than the
// device kiosk's Chromium 68 (owner-confirmed; see
// docs/research/architecture-proposals/frontend.md section 1.6).
//
// Two layers:
//  1. A token scan (own lexer: skips comments, strings, templates and regex
//     literals) for banned syntax and APIs. Always runs.
//  2. A real parse with acorn at ecmaVersion 2019 when acorn can be resolved
//     offline (ACORN_PATH, `require('acorn')`, or a node_modules found by
//     walking up from this repo). acorn is NOT a dependency of this repo; if
//     it is not found layer 2 is skipped and only the token scan guards.
// Plus `node --check` on every source and a CSS scan for properties Chromium
// 68 lacks. Excluded: vendor/ (third-party builds), tests/ (run under node).
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const FRONTEND = path.resolve(__dirname, '..');

// ---- lexer -----------------------------------------------------------------
const PUNCT3 = ['>>>', '...', '===', '!==', '**=', '<<=', '>>=', '&&=', '||=', '??='];
const PUNCT2 = ['=>', '==', '!=', '<=', '>=', '&&', '||', '??', '?.', '++', '--', '+=', '-=', '*=', '/=', '%=', '&=', '|=', '^=', '<<', '>>', '**'];

function lex(src) {
  const toks = [];
  let i = 0;
  let line = 1;
  const n = src.length;
  const stack = []; // template-literal brace depth tracking
  const isIdStart = (c) => /[A-Za-z_$\u00a0-\uffff]/.test(c);
  const isId = (c) => /[A-Za-z0-9_$\u00a0-\uffff]/.test(c);
  const push = (type, value) => toks.push({ type, value, line });
  const regexAllowed = () => {
    const p = toks[toks.length - 1];
    if (!p) return true;
    if (p.type === 'num' || p.type === 'str' || p.type === 'regex') return false;
    if (p.type === 'ident') return ['return', 'typeof', 'case', 'in', 'of', 'delete', 'void', 'throw', 'new', 'else', 'do', 'yield', 'await'].indexOf(p.value) !== -1;
    return [')', ']', '}'].indexOf(p.value) === -1;
  };
  function readTemplate() {
    // positioned after the opening backtick or a closing `}` of ${...}
    while (i < n) {
      const c = src[i];
      if (c === '\\') { i += 2; continue; }
      if (c === '\n') line++;
      if (c === '`') { i++; push('str', '`'); return; }
      if (c === '$' && src[i + 1] === '{') { i += 2; push('punct', '${'); stack.push(0); return; }
      i++;
    }
  }
  while (i < n) {
    const c = src[i];
    if (c === '\n') { line++; i++; continue; }
    if (/\s/.test(c)) { i++; continue; }
    if (c === '/' && src[i + 1] === '/') { while (i < n && src[i] !== '\n') i++; continue; }
    if (c === '/' && src[i + 1] === '*') {
      i += 2;
      while (i < n && !(src[i] === '*' && src[i + 1] === '/')) { if (src[i] === '\n') line++; i++; }
      i += 2;
      continue;
    }
    if (c === '#' && i === 0 && src[1] === '!') { while (i < n && src[i] !== '\n') i++; continue; }
    if (c === '"' || c === "'") {
      i++;
      while (i < n && src[i] !== c) { if (src[i] === '\\') i++; if (src[i] === '\n') line++; i++; }
      i++;
      push('str', c);
      continue;
    }
    if (c === '`') { i++; readTemplate(); continue; }
    if (c === '}' && stack.length && stack[stack.length - 1] === 0) {
      stack.pop(); push('punct', '}'); i++; readTemplate(); continue;
    }
    if (c === '/' && regexAllowed()) {
      i++;
      let inClass = false;
      while (i < n && (src[i] !== '/' || inClass)) {
        if (src[i] === '\\') i++;
        else if (src[i] === '[') inClass = true;
        else if (src[i] === ']') inClass = false;
        i++;
      }
      i++;
      while (i < n && isId(src[i])) i++;
      push('regex', '/');
      continue;
    }
    if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(src[i + 1] || ''))) {
      while (i < n && /[0-9A-Za-z_.]/.test(src[i])) {
        if ((src[i] === 'e' || src[i] === 'E') && (src[i + 1] === '+' || src[i + 1] === '-')) i++;
        i++;
      }
      push('num', '0');
      continue;
    }
    if (isIdStart(c)) {
      let j = i;
      while (j < n && isId(src[j])) j++;
      push('ident', src.slice(i, j));
      i = j;
      continue;
    }
    if (c === '#') { push('punct', '#'); i++; continue; }
    const t3 = src.substr(i, 3);
    if (PUNCT3.indexOf(t3) !== -1) { push('punct', t3); i += 3; continue; }
    const t2 = src.substr(i, 2);
    if (t2 === '?.' && /[0-9]/.test(src[i + 2] || '')) { push('punct', '?'); i++; continue; } // ternary with a decimal: a ? .5 : 1
    if (PUNCT2.indexOf(t2) !== -1) { push('punct', t2); i += 2; continue; }
    if (c === '{' && stack.length) stack[stack.length - 1]++;
    if (c === '}' && stack.length) stack[stack.length - 1]--;
    push('punct', c);
    i++;
  }
  return toks;
}

// ---- rules -----------------------------------------------------------------
const BANNED_METHODS = {
  flat: 'Array.prototype.flat (Chromium 69)',
  flatMap: 'Array.prototype.flatMap (Chromium 69)',
  matchAll: 'String.prototype.matchAll (Chromium 73)',
  fromEntries: 'Object.fromEntries (Chromium 73)',
  allSettled: 'Promise.allSettled (Chromium 76)',
  replaceAll: 'String.prototype.replaceAll (Chromium 85)',
  at: 'Array/String.prototype.at (Chromium 92)',
};
const BANNED_GLOBALS = {
  globalThis: 'globalThis (Chromium 71)',
  structuredClone: 'structuredClone (Chromium 98)',
};

const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k);

function scanJs(src) {
  const toks = lex(src);
  const found = [];
  const add = (t, what) => found.push('line ' + t.line + ': ' + what);
  for (let k = 0; k < toks.length; k++) {
    const t = toks[k];
    const next = toks[k + 1];
    const prev = toks[k - 1];
    if (t.type === 'punct') {
      if (t.value === '?.') add(t, 'optional chaining ?. (Chromium 80)');
      if (t.value === '??' || t.value === '??=') add(t, 'nullish coalescing ' + t.value + ' (Chromium 80)');
      if (t.value === '&&=' || t.value === '||=') add(t, 'logical assignment ' + t.value + ' (Chromium 85)');
      if (t.value === '#' && next && next.type === 'ident') add(t, 'private class member #' + next.value + ' (Chromium 74)');
    }
    if (t.type === 'ident') {
      if (has(BANNED_GLOBALS, t.value) && !(prev && prev.value === '.')) add(t, BANNED_GLOBALS[t.value]);
      if (has(BANNED_METHODS, t.value) && prev && prev.value === '.' && next && next.value === '(') add(t, BANNED_METHODS[t.value]);
      if (t.value === 'class' && !(prev && prev.value === '.')) scanClass(toks, k, add);
    }
  }
  return found;
}

// Walks a class body; any member that is not a method/accessor is a field.
function scanClass(toks, k, add) {
  let i = k + 1;
  while (i < toks.length && toks[i].value !== '{') {
    if (toks[i].value === '(') i = skipBalanced(toks, i); else i++;
  }
  i++; // into body
  while (i < toks.length && toks[i].value !== '}') {
    if (toks[i].value === ';') { i++; continue; }
    const start = toks[i];
    if (toks[i].value === 'static' && toks[i + 1] && toks[i + 1].value === '{') { add(start, 'static block (Chromium 94)'); i = skipBalanced(toks, i + 1); continue; }
    while (toks[i] && ['static', 'async', 'get', 'set', '*'].indexOf(toks[i].value) !== -1 && toks[i + 1] && toks[i + 1].value !== '(' && toks[i + 1].value !== '=' && toks[i + 1].value !== ';') i++;
    if (toks[i] && toks[i].value === '[') i = skipBalanced(toks, i); else i++; // member name
    if (toks[i] && toks[i].value === '(') {
      i = skipBalanced(toks, i); // params
      if (toks[i] && toks[i].value === '{') i = skipBalanced(toks, i); // body
    } else {
      add(start, 'class field (Chromium 72)');
      while (i < toks.length && toks[i].value !== ';' && toks[i].value !== '}') {
        if (['(', '[', '{'].indexOf(toks[i].value) !== -1) i = skipBalanced(toks, i); else i++;
      }
    }
  }
}

function skipBalanced(toks, i) {
  const open = toks[i].value;
  const close = { '(': ')', '[': ']', '{': '}' }[open];
  let depth = 0;
  for (; i < toks.length; i++) {
    if (toks[i].value === open) depth++;
    else if (toks[i].value === close) { depth--; if (depth === 0) return i + 1; }
  }
  return i;
}

const CSS_BANNED = [
  [/(^|[;{\s])inset\s*:/, 'inset shorthand (Chromium 87); use top/right/bottom/left'],
  [/aspect-ratio\s*:/, 'aspect-ratio (Chromium 88)'],
  [/backdrop-filter\s*:/, 'backdrop-filter (Chromium 76)'],
  [/:is\(|:where\(/, ':is()/:where() (Chromium 88)'],
  [/:focus-visible/, ':focus-visible (Chromium 86)'],
  [/prefers-color-scheme/, 'prefers-color-scheme (Chromium 76)'],
  [/[:\s,(]clamp\(/, 'clamp() (Chromium 79)'],
  [/[:\s,(](min|max)\(/, 'CSS min()/max() (Chromium 79)'],
];

function scanCss(src) {
  const text = src.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '));
  const found = [];
  const lineOf = (idx) => text.slice(0, idx).split('\n').length;
  CSS_BANNED.forEach((rule) => {
    const m = rule[0].exec(text);
    if (m) found.push('line ' + lineOf(m.index) + ': ' + rule[1]);
  });
  const blockRe = /([^{}]+)\{([^{}]*)\}/g;
  let b;
  while ((b = blockRe.exec(text))) {
    const body = b[2];
    if (/display\s*:\s*(inline-)?flex/.test(body) && /(^|[;\s])(row-|column-)?gap\s*:/.test(body)) {
      found.push('line ' + lineOf(b.index + b[1].length) + ': flexbox gap in "' + b[1].trim() + '" (Chromium 84); use margins');
    }
  }
  return found;
}

// ---- file discovery -----------------------------------------------------------
function walk(dir, out) {
  fs.readdirSync(dir, { withFileTypes: true }).forEach((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (['vendor', 'tests', 'node_modules'].indexOf(e.name) !== -1) return;
      walk(p, out);
    } else out.push(p);
  });
  return out;
}

const files = walk(FRONTEND, []);
const jsFiles = files.filter((f) => f.endsWith('.js'));
const cssFiles = files.filter((f) => f.endsWith('.css'));
const htmlFiles = files.filter((f) => f.endsWith('.html'));
const rel = (f) => path.relative(FRONTEND, f);

function resolveAcorn() {
  const tries = [];
  if (process.env.ACORN_PATH) tries.push(process.env.ACORN_PATH);
  tries.push('acorn');
  let dir = FRONTEND;
  for (let up = 0; up < 6; up++) { tries.push(path.join(dir, 'node_modules', 'acorn')); dir = path.dirname(dir); }
  try {
    const g = execFileSync('npm', ['root', '-g'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    tries.push(path.join(g, 'acorn'));
  } catch (e) { /* no npm */ }
  for (const t of tries) {
    try { const a = require(t); if (a && typeof a.parse === 'function') return a; } catch (e) { /* next */ }
  }
  return null;
}

// ---- tests ---------------------------------------------------------------------
test('scanner self-test: every banned construct is detected, lookalikes are not', () => {
  const bad = {
    'a?.b': 'optional chaining', 'a ?? b': 'nullish', 'x ??= 1': 'nullish', 'class A { x = 1; }': 'class field',
    'class A { static y = 2 }': 'class field', 'class A { #p = 1 }': 'private', 'class A { m() { return this.#p; } }': 'private',
    'arr.flat()': 'flat', 'arr.flatMap(f)': 'flatMap', 's.matchAll(/a/g)': 'matchAll', 'Object.fromEntries(e)': 'fromEntries',
    'globalThis.x': 'globalThis', 'Promise.allSettled(p)': 'allSettled', 's.replaceAll("a","b")': 'replaceAll', 'a.at(-1)': 'at(',
    'structuredClone(o)': 'structuredClone', 'a ||= b': 'logical assignment',
    '`${a?.b}`': 'optional chaining in a template',
  };
  Object.keys(bad).forEach((code) => assert.ok(scanJs(code).length > 0, 'missed: ' + code));
  const ok = [
    'const a = b ? .5 : 1;', 'a ? b : c', 'var s = "a?.b ?? c"; // x?.y', '/* a?.b */ var x = 1;', 'var r = /a?.b/;',
    'class A { constructor() { this.x = 1; } get y() { return 1; } static s() {} async m() {} *g() {} [k]() {} }',
    'o.flat = 1;', 'var at = 1; at + 1', 'x = { at: 1, flat: 2 }', 'const t = `a${b}c`;', 'async function f() { try { await g(); } catch { } }',
    'var o = { ...a, b };', 'const f = async () => { for await (const x of y) {} };',
  ];
  ok.forEach((code) => assert.deepEqual(scanJs(code), [], 'false positive: ' + code + ' -> ' + scanJs(code).join(';')));
  assert.ok(scanCss('.a { display: flex; gap: 4px; }').length > 0);
  assert.ok(scanCss('.a { position: absolute; inset: 0; }').length > 0);
  assert.ok(scanCss('.a { width: clamp(1px, 2vw, 3px); }').length > 0);
  assert.deepEqual(scanCss('.a { display: grid; gap: 4px; } /* display: flex; gap: 1px */ .b { display: flex; margin: 0; }'), []);
});

test('sources are discovered (guard is not vacuous)', () => {
  assert.ok(jsFiles.length >= 3, 'js files: ' + jsFiles.map(rel).join(', '));
  assert.ok(jsFiles.some((f) => rel(f).indexOf('src' + path.sep) === 0) || jsFiles.some((f) => rel(f) === 'app.js'));
  assert.ok(cssFiles.length >= 1);
  assert.ok(files.every((f) => rel(f).indexOf('vendor' + path.sep) !== 0));
});

test('node --check passes for every shipped script', () => {
  jsFiles.forEach((f) => {
    try { execFileSync(process.execPath, ['--check', f], { stdio: 'pipe' }); }
    catch (e) { assert.fail(rel(f) + ': ' + String(e.stderr || e.message).split('\n').slice(0, 4).join(' | ')); }
  });
});

test('no syntax or API newer than Chromium 68 in shipped scripts (token scan)', () => {
  const problems = [];
  jsFiles.forEach((f) => scanJs(fs.readFileSync(f, 'utf8')).forEach((p) => problems.push(rel(f) + ' ' + p)));
  assert.deepEqual(problems, []);
});

test('inline scripts in html files obey the same rules', () => {
  const problems = [];
  htmlFiles.forEach((f) => {
    const re = /<script(?![^>]*\ssrc=)[^>]*>([\s\S]*?)<\/script>/g;
    let m;
    while ((m = re.exec(fs.readFileSync(f, 'utf8')))) scanJs(m[1]).forEach((p) => problems.push(rel(f) + ' ' + p));
  });
  assert.deepEqual(problems, []);
});

test('acorn parse at ES2019 (skipped when acorn is not resolvable offline)', (t) => {
  const acorn = resolveAcorn();
  if (!acorn) { t.skip('acorn not found offline; token scan only'); return; }
  const problems = [];
  jsFiles.forEach((f) => {
    try { acorn.parse(fs.readFileSync(f, 'utf8'), { ecmaVersion: 2019, sourceType: 'script', allowHashBang: true }); }
    catch (e) { problems.push(rel(f) + ': ' + e.message); }
  });
  assert.deepEqual(problems, []);
});

test('no CSS newer than Chromium 68 (flexbox gap, inset, clamp, aspect-ratio, ...)', () => {
  const problems = [];
  cssFiles.forEach((f) => scanCss(fs.readFileSync(f, 'utf8')).forEach((p) => problems.push(rel(f) + ' ' + p)));
  assert.deepEqual(problems, []);
});
