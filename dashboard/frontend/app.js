/*
 * NOC dashboard frontend. Deliberately plain ES6 (arrow functions, let/const,
 * template literals, fetch, async/await) since those all run fine on the
 * device's on-board Chromium 69 -- but no ES2020 syntax (optional chaining
 * `?.`, nullish coalescing `??`), which Chromium 69 silently fails to parse
 * with no console error at all (confirmed the hard way in the sibling
 * bridge/ui prototype, see docs/getting-started.md).
 */

const METRICS_URL = 'metrics.json';
const POLL_MS = 2000;
const GAUGE_CIRCUMFERENCE = 314; // 2 * PI * r, r=50, matches the SVG circle

const RING_IDS = { cpu: 'cpu-ring', mem: 'mem-ring' };

function levelColor(pct) {
  if (pct >= 85) return 'var(--red)';
  if (pct >= 60) return 'var(--amber)';
  return 'var(--green)';
}

function levelGlow(pct) {
  if (pct >= 85) return 'var(--red-glow)';
  if (pct >= 60) return 'var(--amber-glow)';
  return 'var(--green-glow)';
}

function setGauge(ringId, numberId, pct) {
  const ring = document.getElementById(ringId);
  const number = document.getElementById(numberId);
  const clamped = Math.max(0, Math.min(100, pct || 0));
  const offset = GAUGE_CIRCUMFERENCE * (1 - clamped / 100);
  ring.style.strokeDashoffset = String(offset);
  ring.style.stroke = levelColor(clamped);
  ring.style.filter = 'drop-shadow(0 0 6px ' + levelGlow(clamped) + ')';
  number.textContent = clamped.toFixed(0);
}

function renderCores(containerId, cores) {
  const el = document.getElementById(containerId);
  el.innerHTML = '';
  cores.forEach(function (pct) {
    const bar = document.createElement('div');
    bar.className = 'core-bar';
    const fill = document.createElement('div');
    fill.className = 'core-bar-fill';
    fill.style.height = Math.max(2, pct) + '%';
    fill.style.background = levelColor(pct);
    bar.appendChild(fill);
    el.appendChild(bar);
  });
}

function drawSparkline(canvasId, series, color) {
  const canvas = document.getElementById(canvasId);
  const ctx = canvas.getContext('2d');
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  if (!series || series.length < 2) return;

  const max = Math.max(10, Math.max.apply(null, series));
  const step = w / (series.length - 1);

  ctx.beginPath();
  series.forEach(function (value, i) {
    const x = i * step;
    const y = h - (value / max) * (h - 4) - 2;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.shadowColor = color;
  ctx.shadowBlur = 4;
  ctx.stroke();

  // fill under the line for a subtle glow-wall look
  ctx.lineTo(w, h);
  ctx.lineTo(0, h);
  ctx.closePath();
  const gradient = ctx.createLinearGradient(0, 0, 0, h);
  gradient.addColorStop(0, color);
  gradient.addColorStop(1, 'rgba(0,0,0,0)');
  ctx.globalAlpha = 0.15;
  ctx.fillStyle = gradient;
  ctx.fill();
  ctx.globalAlpha = 1;
}

function renderMemBreakdown(mem) {
  const el = document.getElementById('mem-breakdown');
  const fmt = function (kb) { return (kb / 1024).toFixed(0) + 'M'; };
  el.innerHTML =
    '<span>avail <span class="val">' + fmt(mem.available_kb) + '</span></span>' +
    '<span>cache <span class="val">' + fmt(mem.cached_kb) + '</span></span>' +
    '<span>buff <span class="val">' + fmt(mem.buffers_kb) + '</span></span>';
}

function renderDiskRows(disks) {
  const el = document.getElementById('disk-rows');
  el.innerHTML = '';
  disks.forEach(function (d) {
    const row = document.createElement('div');
    row.className = 'disk-row';

    const mount = document.createElement('div');
    mount.className = 'disk-mount';
    mount.textContent = d.mount;

    const track = document.createElement('div');
    track.className = 'disk-bar-track';
    const fill = document.createElement('div');
    fill.className = 'disk-bar-fill';
    fill.style.width = d.pct + '%';
    fill.style.background = levelColor(d.pct);
    track.appendChild(fill);

    const size = document.createElement('div');
    size.className = 'disk-size';
    size.textContent = (d.used_kb / 1024).toFixed(0) + 'M / ' + (d.size_kb / 1024).toFixed(0) + 'M (' + d.pct + '%)';

    row.appendChild(mount);
    row.appendChild(track);
    row.appendChild(size);
    el.appendChild(row);
  });
}

function renderBluetooth(bt) {
  const dot = document.getElementById('bt-dot');
  const text = document.getElementById('bt-text');
  const name = document.getElementById('bt-name');
  if (!bt || !bt.present) {
    dot.classList.remove('up');
    text.textContent = 'ABSENT';
    name.textContent = 'hci0 not found';
    return;
  }
  if (bt.up) {
    dot.classList.add('up');
    text.textContent = 'UP';
  } else {
    dot.classList.remove('up');
    text.textContent = 'DOWN';
  }
  name.textContent = bt.name ? bt.name : 'hci0';
}

function renderNet(ifaces) {
  const el = document.getElementById('net-list');
  if (!ifaces || ifaces.length === 0) {
    el.innerHTML = '<div class="status-sub">no active interfaces (bnep0/usb0 not present)</div>';
    return;
  }
  el.innerHTML = '';
  ifaces.forEach(function (n) {
    const row = document.createElement('div');
    row.className = 'net-row';
    const dot = document.createElement('span');
    dot.className = 'status-dot' + (n.state === 'up' ? ' up' : '');
    const label = document.createElement('span');
    label.textContent = n.iface + ' / ' + n.state;
    row.appendChild(dot);
    row.appendChild(label);
    el.appendChild(row);
  });
}

function updateClock() {
  const el = document.getElementById('clock');
  const now = new Date();
  const pad = function (n) { return (n < 10 ? '0' : '') + n; };
  el.textContent = pad(now.getHours()) + ':' + pad(now.getMinutes()) + ':' + pad(now.getSeconds());
}

function setLink(ok) {
  const dot = document.getElementById('link-dot');
  const text = document.getElementById('link-text');
  if (ok) {
    dot.classList.add('ok');
    text.textContent = 'LIVE';
  } else {
    dot.classList.remove('ok');
    text.textContent = 'NO SIGNAL';
  }
}

async function poll() {
  try {
    const res = await fetch(METRICS_URL + '?t=' + Date.now(), { cache: 'no-store' });
    if (!res.ok) throw new Error('bad status ' + res.status);
    const data = await res.json();
    render(data);
    setLink(true);
  } catch (err) {
    setLink(false);
  }
}

function render(data) {
  setGauge(RING_IDS.cpu, 'cpu-pct', data.cpu.total_pct);
  renderCores('cpu-cores', data.cpu.per_core || []);
  drawSparkline('cpu-spark', data.cpu.history, '#35e38a');

  setGauge(RING_IDS.mem, 'mem-pct', data.mem.pct);
  renderMemBreakdown(data.mem);
  drawSparkline('mem-spark', data.mem.history, '#35e38a');

  renderDiskRows(data.disk || []);

  const mmc = (data.io && data.io.mmcblk0) || { read_kb_s: 0, write_kb_s: 0 };
  document.getElementById('io-read').textContent = mmc.read_kb_s.toFixed(1);
  document.getElementById('io-write').textContent = mmc.write_kb_s.toFixed(1);
  const ioHistory = (data.io_history && data.io_history.read_kb_s) || [];
  drawSparkline('io-spark', ioHistory, '#3fd4ff');

  const load = data.load || [0, 0, 0];
  document.getElementById('load-1').textContent = load[0].toFixed(2);
  document.getElementById('load-5').textContent = load[1].toFixed(2);
  document.getElementById('load-15').textContent = load[2].toFixed(2);
  document.getElementById('proc-count').textContent = data.proc_count;

  renderBluetooth(data.bluetooth);
  renderNet(data.net);

  const ts = new Date(data.ts);
  const pad = function (n) { return (n < 10 ? '0' : '') + n; };
  document.getElementById('last-update').textContent =
    'last update: ' + pad(ts.getHours()) + ':' + pad(ts.getMinutes()) + ':' + pad(ts.getSeconds());
}

setInterval(updateClock, 1000);
updateClock();
poll();
setInterval(poll, POLL_MS);
