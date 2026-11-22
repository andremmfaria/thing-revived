/*
 * Status bar clock: the agent's clock_sync offset applied to the local time.
 * formatClock and dotClasses are pure; start() wires the redraw interval.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.clock).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  function pad(n) { return (n < 10 ? '0' : '') + n; }

  // format: 'HH:mm:ss' | 'HH:mm' (24 hour, local time of the shifted instant).
  function formatClock(nowMs, offsetMs, format) {
    var d = new Date(nowMs + offsetMs);
    var text = pad(d.getHours()) + ':' + pad(d.getMinutes());
    return format === 'HH:mm' ? text : text + ':' + pad(d.getSeconds());
  }

  // Dot state for a clock_sync status: lit when ok, amber when stale or unsynced.
  function dotClasses(status) {
    return { ok: status === 'ok', stale: status === 'stale' || status === 'unsynced' };
  }

  // deps: { doc, timers, getClock() -> {offsetMs, status}, format, intervalMs }
  function create(deps) {
    function render() {
      var c = deps.getClock();
      var dot = deps.doc.getElementById('clock-dot');
      var cls = dotClasses(c.status);
      dot.classList.toggle('ok', cls.ok);
      dot.classList.toggle('stale', cls.stale);
      deps.doc.getElementById('clock-text').textContent = formatClock(deps.timers.now(), c.offsetMs, deps.format);
    }
    return {
      render: render,
      start: function () { return deps.timers.setInterval(render, deps.intervalMs); },
    };
  }

  var api = { formatClock: formatClock, dotClasses: dotClasses, create: create };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.clock = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
