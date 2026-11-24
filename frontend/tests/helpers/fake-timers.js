// Deterministic stand-in for core/timers.js: time only moves with tick().
'use strict';

function createFakeTimers() {
  let now = 1000;
  let nextId = 1;
  const pending = new Map(); // id -> {at, fn, every}
  const t = {
    now: () => now,
    mono: () => now,
    setTimeout(fn, ms) { const id = nextId++; pending.set(id, { at: now + ms, fn, every: 0 }); return id; },
    clearTimeout(id) { pending.delete(id); },
    setInterval(fn, ms) { const id = nextId++; pending.set(id, { at: now + ms, fn, every: ms }); return id; },
    clearInterval(id) { pending.delete(id); },
    pending: () => pending.size,
    tick(ms) {
      const end = now + ms;
      for (;;) {
        let best = null;
        pending.forEach((p, id) => { if (p.at <= end && (best === null || p.at < best.p.at)) best = { id, p }; });
        if (!best) break;
        now = best.p.at;
        if (best.p.every) best.p.at += best.p.every; else pending.delete(best.id);
        best.p.fn();
      }
      now = end;
    },
  };
  return t;
}

module.exports = { createFakeTimers };
