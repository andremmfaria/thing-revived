// Test-only reader for the TOML subset used by config/frontend.toml:
// [section] headers, `key = value` lines, # comments, and values that are
// basic strings, integers, floats, booleans or one-line arrays of strings.
// Anything else throws, so a config file that outgrows the subset fails the
// tests loudly instead of being half parsed. Not shipped to the device.
'use strict';

function parseString(text, line) {
  // text starts at the opening quote; returns [value, restAfterClosingQuote]
  let out = '';
  let i = 1;
  while (i < text.length) {
    const c = text[i];
    if (c === '\\') {
      const n = text[i + 1];
      const map = { n: '\n', t: '\t', '"': '"', '\\': '\\', r: '\r' };
      if (!(n in map)) throw new Error('toml line ' + line + ': unsupported escape \\' + n);
      out += map[n];
      i += 2;
    } else if (c === '"') {
      return [out, text.slice(i + 1)];
    } else {
      out += c;
      i++;
    }
  }
  throw new Error('toml line ' + line + ': unterminated string');
}

function parseValue(text, line) {
  text = text.trim();
  if (text[0] === '"') {
    const r = parseString(text, line);
    const rest = r[1].trim();
    if (rest && rest[0] !== '#') throw new Error('toml line ' + line + ': trailing text after string');
    return r[0];
  }
  if (text[0] === '[') {
    const items = [];
    let rest = text.slice(1).trim();
    while (rest[0] !== ']') {
      if (rest[0] !== '"') throw new Error('toml line ' + line + ': arrays may only hold strings');
      const r = parseString(rest, line);
      items.push(r[0]);
      rest = r[1].trim();
      if (rest[0] === ',') rest = rest.slice(1).trim();
      else if (rest[0] !== ']') throw new Error('toml line ' + line + ': bad array');
    }
    const tail = rest.slice(1).trim();
    if (tail && tail[0] !== '#') throw new Error('toml line ' + line + ': trailing text after array');
    return items;
  }
  const bare = text.split('#')[0].trim();
  if (bare === 'true') return true;
  if (bare === 'false') return false;
  if (/^[+-]?\d+$/.test(bare)) return parseInt(bare, 10);
  if (/^[+-]?\d+\.\d+$/.test(bare)) return parseFloat(bare);
  throw new Error('toml line ' + line + ': unsupported value ' + bare);
}

function parse(source) {
  const out = {};
  let section = null;
  source.split(/\r?\n/).forEach((raw, idx) => {
    const line = raw.trim();
    const n = idx + 1;
    if (!line || line[0] === '#') return;
    const header = /^\[([A-Za-z0-9_]+)\]\s*(#.*)?$/.exec(line);
    if (header) {
      section = header[1];
      if (out[section]) throw new Error('toml line ' + n + ': duplicate section ' + section);
      out[section] = {};
      return;
    }
    const kv = /^([A-Za-z0-9_]+)\s*=\s*(.+)$/.exec(line);
    if (!kv || !section) throw new Error('toml line ' + n + ': cannot parse "' + line + '"');
    if (kv[1] in out[section]) throw new Error('toml line ' + n + ': duplicate key ' + kv[1]);
    out[section][kv[1]] = parseValue(kv[2], n);
  });
  return out;
}

// Pairs each key with the comment line directly above it, for the "every key
// is documented" check. Returns [{section, key, comment}].
function documentedKeys(source) {
  const res = [];
  let section = null;
  let prev = '';
  source.split(/\r?\n/).forEach((raw) => {
    const line = raw.trim();
    const header = /^\[([A-Za-z0-9_]+)\]/.exec(line);
    if (header) { section = header[1]; prev = ''; return; }
    const kv = /^([A-Za-z0-9_]+)\s*=/.exec(line);
    if (kv) res.push({ section, key: kv[1], comment: prev });
    prev = line[0] === '#' ? line : '';
  });
  return res;
}

module.exports = { parse, documentedKeys };
