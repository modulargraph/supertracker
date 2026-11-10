'use strict';
// host-transport.js: channel selection (web view, WebSocket, none) and what each channel guarantees.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../ui/host-transport.js'), 'utf8');

function load(window) {
  const context = vm.createContext({ window, URLSearchParams, console: { error() {} }, Promise, JSON, Set, Error });
  vm.runInContext(source, context, { filename: 'host-transport.js' });
  return window.STHostTransport;
}
const tick = () => new Promise(resolve => setImmediate(resolve));

class FakeSocket {
  constructor(url) { this.url = url; this.sent = []; FakeSocket.last = this; }
  send(text) { this.sent.push(text); }
  open() { this.onopen(); }
  message(value) { this.onmessage({ data: typeof value === 'string' ? value : JSON.stringify(value) }); }
}

(async () => {
  {
    const transport = load({});
    assert.equal(transport.kind, 'none'); assert.equal(transport.available, false);
    assert.throws(() => transport.post({ id: 1, command: 'ready' }), /No SuperTracker host/);
    // A file:// page, or a served page without a token, has no host either.
    assert.equal(load({ location: { protocol: 'file:', host: '', search: '' }, WebSocket: FakeSocket }).available, false);
    assert.equal(load({ location: { protocol: 'http:', host: '127.0.0.1:8123', search: '?other=1' }, WebSocket: FakeSocket }).available, false);
    console.log('PASS a page opened without a host has no transport');
  }
  {
    const posted = [], window = { ipc: { postMessage: text => posted.push(text) }, location: { protocol: 'http:', host: 'x', search: '?token=t' }, WebSocket: FakeSocket };
    FakeSocket.last = null;
    const transport = load(window), seen = [];
    assert.equal(transport.kind, 'webview'); assert.equal(transport.available, true);
    assert.equal(FakeSocket.last, null, 'the web view channel wins over the WebSocket');
    transport.post({ id: 7, command: 'project.save', payload: { path: '/tmp/a' } });
    assert.deepEqual(posted, ['{"id":7,"command":"project.save","payload":{"path":"/tmp/a"}}']);
    const stop = transport.onMessage(message => seen.push(message));
    transport.onMessage(() => { throw new Error('a broken listener'); });
    transport.receive({ id: 7, ok: true, result: { saved: true } });
    transport.receive('{"type":"status","payload":{"playing":false}}');
    transport.receive('not json'); transport.receive(null); transport.receive(42);
    assert.deepEqual(seen, [{ id: 7, ok: true, result: { saved: true } }, { type: 'status', payload: { playing: false } }]);
    stop(); transport.receive({ type: 'later', payload: {} });
    assert.equal(seen.length, 2);
    console.log('PASS the web view channel posts JSON texts and delivers objects or texts to every listener');
  }
  {
    const window = { location: { protocol: 'http:', host: '127.0.0.1:8123', search: '?token=a%2Fb+c' }, WebSocket: FakeSocket };
    const transport = load(window), socket = FakeSocket.last, seen = [];
    assert.equal(transport.kind, 'websocket'); assert.equal(transport.available, true);
    assert.equal(socket.url, 'ws://127.0.0.1:8123/bridge?token=a%2Fb%20c');
    assert.equal(transport.socketURL({ protocol: 'https:', host: 'studio.example', search: '?token=t' }), 'wss://studio.example/bridge?token=t');
    transport.onMessage(message => seen.push(message));
    transport.post({ id: 1, command: 'ready', payload: {} }); transport.post({ id: 2, command: 'engine.project', payload: {} });
    assert.deepEqual(socket.sent, [], 'requests wait for the socket to open');
    socket.open();
    assert.deepEqual(socket.sent.map(text => JSON.parse(text).id), [1, 2]);
    transport.post({ id: 3, command: 'transport.play', payload: {} });
    assert.equal(socket.sent.length, 3);
    socket.message({ id: 1, ok: true, result: { version: '0.1.0' } }); socket.message({ type: 'status', payload: {} }); socket.message('garbage');
    assert.deepEqual(seen, [{ id: 1, ok: true, result: { version: '0.1.0' } }, { type: 'status', payload: {} }]);
    // The host goes away: the two requests still waiting are answered, once, and the page is told.
    seen.length = 0; socket.onclose(); socket.onerror();
    assert.deepEqual(seen.map(message => message.id ?? message.type), [2, 3, 'error']);
    assert.ok(seen.every(message => message.type === 'error' ? /connection .* was lost/.test(message.payload.message) : message.ok === false && /was lost/.test(message.error)));
    seen.length = 0; transport.post({ id: 4, command: 'transport.stop', payload: {} });
    assert.equal(socket.sent.length, 3); assert.deepEqual(seen, []);
    await tick();
    assert.equal(seen.length, 1); assert.equal(seen[0].id, 4); assert.equal(seen[0].ok, false);
    console.log('PASS the WebSocket channel queues until open and answers every waiting request when the host is lost');
  }
  {
    const window = { location: { protocol: 'http:', host: '127.0.0.1:8123', search: '?token=wrong' }, WebSocket: FakeSocket };
    const transport = load(window), seen = [];
    transport.onMessage(message => seen.push(message));
    transport.post({ id: 1, command: 'ready', payload: {} });
    FakeSocket.last.onerror(); FakeSocket.last.onclose();
    assert.equal(seen.length, 2); assert.match(seen[0].error, /did not accept the connection/);
    console.log('PASS a refused connection (wrong token) fails the handshake instead of leaving it waiting');
  }
  {
    const provided = { kind: 'test', available: true }, window = { STHostTransport: provided, ipc: { postMessage() {} } };
    assert.equal(load(window), provided);
    console.log('PASS a transport installed by the embedder is kept');
  }
})().catch(error => { console.error(error); process.exit(1); });
