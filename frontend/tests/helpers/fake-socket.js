// Controllable WebSocket stand-in for transport tests.
'use strict';

function createFakeSocketClass() {
  class FakeSocket {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.sent = [];
      FakeSocket.instances.push(this);
    }
    send(data) {
      if (this.readyState !== 1) throw new Error('InvalidStateError');
      this.sent.push(JSON.parse(data));
    }
    close() { this.readyState = 3; }
    // test controls
    open() { this.readyState = 1; if (this.onopen) this.onopen({}); }
    message(obj) { if (this.onmessage) this.onmessage({ data: typeof obj === 'string' ? obj : JSON.stringify(obj) }); }
    error() { if (this.onerror) this.onerror({}); }
    serverClose() { this.readyState = 3; if (this.onclose) this.onclose({}); }
  }
  FakeSocket.instances = [];
  FakeSocket.OPEN = 1;
  return FakeSocket;
}

module.exports = { createFakeSocketClass };
