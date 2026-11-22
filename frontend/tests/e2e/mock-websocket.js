/*
 * In-page WebSocket mock for the headless regression run. Injected with
 * page.addInitScript() BEFORE the app loads, so `new WebSocket(url)` in the
 * app resolves to this class. The test harness (mock-server.mjs) drives it
 * through window.__mock.
 *
 * Connections open on the next tick unless the URL is marked refused.
 * Everything the app sends is recorded in __mock.sent as parsed JSON.
 */
(function () {
  'use strict';
  var sockets = [];
  var sent = [];
  var refused = {};
  var opens = {};

  function MockWebSocket(url) {
    this.url = url;
    this.readyState = 0;
    this.onopen = null;
    this.onclose = null;
    this.onerror = null;
    this.onmessage = null;
    this.id = sockets.length;
    sockets.push(this);
    var self = this;
    setTimeout(function () {
      if (refused[url]) {
        self.readyState = 3;
        if (self.onerror) self.onerror({});
        if (self.onclose) self.onclose({});
        return;
      }
      self.readyState = 1;
      opens[url] = (opens[url] || 0) + 1;
      if (self.onopen) self.onopen({});
    }, 0);
  }
  MockWebSocket.CONNECTING = 0;
  MockWebSocket.OPEN = 1;
  MockWebSocket.CLOSING = 2;
  MockWebSocket.CLOSED = 3;
  MockWebSocket.prototype.send = function (data) {
    if (this.readyState !== 1) throw new Error('InvalidStateError: socket not open');
    sent.push({ url: this.url, socket: this.id, msg: JSON.parse(data), at: Date.now() });
  };
  MockWebSocket.prototype.close = function () {
    if (this.readyState === 3) return;
    this.readyState = 3;
    var self = this;
    setTimeout(function () { if (self.onclose) self.onclose({}); }, 0);
  };

  function latest(url) {
    for (var i = sockets.length - 1; i >= 0; i--) {
      if (sockets[i].url === url && sockets[i].readyState === 1) return sockets[i];
    }
    return null;
  }

  window.WebSocket = MockWebSocket;
  window.__mock = {
    sent: sent,
    sockets: sockets,
    opens: function (url) { return opens[url] || 0; },
    live: function (url) { return latest(url) !== null; },
    // server -> app frame
    deliver: function (url, obj) {
      var s = latest(url);
      if (!s) throw new Error('no live socket for ' + url);
      if (s.onmessage) s.onmessage({ data: typeof obj === 'string' ? obj : JSON.stringify(obj) });
    },
    // server drops the connection
    drop: function (url) {
      var s = latest(url);
      if (!s) return;
      s.readyState = 3;
      if (s.onclose) s.onclose({});
    },
    refuse: function (url, on) { refused[url] = !!on; },
  };
})();
