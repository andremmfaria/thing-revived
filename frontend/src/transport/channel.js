/*
 * One reconnecting JSON WebSocket. The WebSocket constructor, the timers and
 * the logger are injected, so node tests drive it with a fake socket and fake
 * time. Replaces the two near-duplicate connect functions of the old app.js.
 *
 * Status values: 'connecting' (before the first open or failure), 'live',
 * 'error' (socket error, always followed by close), 'disconnected'.
 *
 * send() never fails silently. It returns:
 *   'sent'    written to the live socket
 *   'queued'  socket down, message kept in a bounded queue (only with
 *             opts.queue, meant for idempotent getters; same-key messages
 *             collapse, the oldest is dropped when full) and flushed on the
 *             next open
 *   'dropped' socket down and not queueable; counted in stats(), logged, and
 *             reported to onDrop listeners unless opts.quiet (self-healing
 *             messages such as resize, whose loss needs no user feedback)
 *
 * Reconnect delay: min(max_ms, initial_ms * factor^attempt). factor 1.0 (the
 * default) is the old fixed 2000 ms retry; the attempt counter resets on open.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.channel).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../utils/types.js') : TR.types;
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  var OPEN = 1;

  // opts: { name, url, WebSocketImpl, timers, backoff:{initial_ms,max_ms,factor},
  //         queueMax, logger }
  function create(opts) {
    var name = opts.name || 'channel';
    var log = (opts.logger || Logging).child(name);
    var Impl = opts.WebSocketImpl;
    var timers = opts.timers;
    var backoff = opts.backoff || { initial_ms: 2000, max_ms: 30000, factor: 1.0 };
    var queueMax = typeof opts.queueMax === 'number' ? opts.queueMax : 16;

    var status = Types.ChannelStatus.connecting;
    var ws = null;
    var attempt = 0;
    var retryTimer = null;
    var closed = false;
    var queue = []; // {key, msg}
    var sentKeys = {}; // queueable keys written since the last open
    var stats = { sent: 0, queued: 0, dropped: 0, overflow: 0, badFrames: 0, reconnects: 0 };
    var messageHandlers = [];
    var statusHandlers = [];
    var dropHandlers = [];

    function setStatus(next) {
      if (status === next) return;
      status = next;
      log.debug('status', next);
      statusHandlers.slice().forEach(function (fn) { fn(next); });
    }

    function delayFor(n) {
      var d = backoff.initial_ms * Math.pow(backoff.factor, n);
      return Math.min(backoff.max_ms, Math.round(d));
    }

    function scheduleReconnect() {
      if (closed) return;
      var delay = delayFor(attempt);
      attempt++;
      log.debug('reconnect in', delay, 'ms (attempt', attempt + ')');
      retryTimer = timers.setTimeout(function () {
        retryTimer = null;
        stats.reconnects++;
        connect();
      }, delay);
    }

    function flushQueue() {
      var pending = queue;
      queue = [];
      pending.forEach(function (item) {
        if (sentKeys[item.key]) return; // the same getter was just sent by an on-open listener
        write(item.msg, item.key);
      });
    }

    function write(msg, key) {
      try {
        ws.send(JSON.stringify(msg));
      } catch (err) {
        log.warn('send threw for', Logging.describe(msg), ':', err);
        return false;
      }
      stats.sent++;
      if (key) sentKeys[key] = true;
      return true;
    }

    function connect() {
      if (closed) return;
      var sock;
      try {
        sock = new Impl(opts.url);
      } catch (err) {
        log.warn('cannot create socket for', opts.url, ':', err);
        setStatus(Types.ChannelStatus.disconnected);
        scheduleReconnect();
        return;
      }
      ws = sock;
      sock.onopen = function () {
        if (ws !== sock) return;
        attempt = 0;
        sentKeys = {};
        setStatus(Types.ChannelStatus.live);
        flushQueue();
      };
      sock.onclose = function () {
        if (ws !== sock) return;
        ws = null;
        setStatus(Types.ChannelStatus.disconnected);
        scheduleReconnect();
      };
      sock.onerror = function () {
        if (ws !== sock) return;
        setStatus(Types.ChannelStatus.error);
      };
      sock.onmessage = function (evt) {
        if (ws !== sock) return;
        var msg;
        try {
          msg = JSON.parse(evt.data);
        } catch (err) {
          stats.badFrames++;
          log.warn('unparseable frame,', String(evt.data).length, 'chars');
          return;
        }
        if (msg === null || typeof msg !== 'object' || typeof msg.type !== 'string') {
          stats.badFrames++;
          log.warn('frame without a type field');
          return;
        }
        messageHandlers.slice().forEach(function (fn) { fn(msg); });
      };
    }

    function send(msg, sendOpts) {
      var o = sendOpts || {};
      if (status === Types.ChannelStatus.live && ws && ws.readyState === OPEN) {
        var key = o.queue ? JSON.stringify(msg) : null;
        if (write(msg, key)) return Types.SendResult.sent;
      }
      if (o.queue && queueMax > 0) {
        var qkey = JSON.stringify(msg);
        for (var i = 0; i < queue.length; i++) {
          if (queue[i].key === qkey) return Types.SendResult.queued;
        }
        queue.push({ key: qkey, msg: msg });
        stats.queued++;
        if (queue.length > queueMax) {
          queue.shift();
          stats.overflow++;
          log.warn('queue full, dropped the oldest queued message');
        }
        return Types.SendResult.queued;
      }
      stats.dropped++;
      log.debug('dropped (not connected):', Logging.describe(msg));
      if (!o.quiet) {
        dropHandlers.slice().forEach(function (fn) { fn(msg); });
      }
      return Types.SendResult.dropped;
    }

    function close() {
      closed = true;
      if (retryTimer !== null) {
        timers.clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (ws) {
        var s = ws;
        ws = null;
        s.close();
      }
      setStatus(Types.ChannelStatus.disconnected);
    }

    return {
      start: connect,
      send: send,
      close: close,
      onMessage: function (fn) { messageHandlers.push(fn); },
      onStatus: function (fn) { statusHandlers.push(fn); },
      onDrop: function (fn) { dropHandlers.push(fn); },
      status: function () { return status; },
      queued: function () { return queue.length; },
      stats: function () { return Object.assign({}, stats); },
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.channel = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
