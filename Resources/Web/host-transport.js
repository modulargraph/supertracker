/* Host transport: the one module that knows how the page reaches the host. Everything else sends
   requests through app-bridge.js and never sees which channel carries them.

     STHostTransport.kind                 'webview' | 'websocket' | 'none'
     STHostTransport.available            false when the page was opened without a host (browser preview)
     STHostTransport.post(message)        send one request object ({id, command, payload})
     STHostTransport.onMessage(handler)   hear replies ({id, ok, result | error}) and events ({type, payload});
                                          returns a function that stops listening
     STHostTransport.receive(message)     entry point for the host (the web view channel calls it)

   Channels, in this order:
   - Provided: an embedder that installed its own STHostTransport before this script ran keeps it (the
     unit tests play the host this way).
   - Web view: inside the application the web view provides window.ipc.postMessage(text), and the host
     delivers by calling STHostTransport.receive(...).
   - WebSocket: a page served by `supertracker --serve` has the access token in its URL (?token=...) and
     connects to ws://<same host>/bridge?token=... Requests sent before the socket is open wait for it;
     when the socket is lost, every request still waiting is answered with an error.
   - None: no host. app-bridge.js then answers with its browser-preview stand-ins. */
(function () {
  'use strict';

  if (window.STHostTransport) return;

  const handlers = new Set();

  function receive(message) {
    if (typeof message === 'string') {
      try {
        message = JSON.parse(message);
      } catch {
        return;
      }
    }
    if (!message || typeof message !== 'object') return;
    for (const handler of [...handlers]) {
      // One listener's failure must not keep a reply from the others, or stop the batch being delivered.
      try {
        handler(message);
      } catch (error) {
        console.error(error);
      }
    }
  }

  function onMessage(handler) {
    handlers.add(handler);
    return () => handlers.delete(handler);
  }

  function webViewChannel() {
    const ipc = window.ipc;
    if (!ipc || typeof ipc.postMessage !== 'function') return null;
    return { kind: 'webview', post: message => ipc.postMessage(JSON.stringify(message)) };
  }

  /** The bridge address for a page URL, or null when the page was not served by a host. */
  function socketURL(location) {
    if (!location || (location.protocol !== 'http:' && location.protocol !== 'https:') || !location.host) return null;
    const token = new URLSearchParams(location.search || '').get('token');
    if (!token) return null;
    return (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/bridge?token=' + encodeURIComponent(token);
  }

  function socketChannel() {
    const url = socketURL(window.location);
    const Socket = window.WebSocket;
    if (!url || typeof Socket !== 'function') return null;
    const LOST = 'The connection to the SuperTracker host was lost. Start it again and reload this page.';
    const waiting = new Set();   // ids of requests without a reply
    let queue = [];              // texts posted before the socket opened
    let state = 'connecting';    // 'connecting' | 'open' | 'lost'
    const socket = new Socket(url);

    function lose() {
      if (state === 'lost') return;
      const opened = state === 'open';
      state = 'lost';
      queue = [];
      const ids = [...waiting];
      waiting.clear();
      for (const id of ids) receive({ id, ok: false, error: opened ? LOST : 'The SuperTracker host did not accept the connection. Open the address it printed when it started.' });
      receive({ type: 'error', payload: { message: LOST } });
    }

    socket.onopen = () => {
      state = 'open';
      for (const text of queue.splice(0)) socket.send(text);
    };
    socket.onmessage = event => {
      let message;
      try {
        message = JSON.parse(event.data);
      } catch {
        return;
      }
      if (message && 'id' in message) waiting.delete(message.id);
      receive(message);
    };
    socket.onclose = lose;
    socket.onerror = lose;

    return {
      kind: 'websocket',
      post(message) {
        if (state === 'lost') {
          Promise.resolve().then(() => receive({ id: message.id, ok: false, error: LOST }));
          return;
        }
        waiting.add(message.id);
        const text = JSON.stringify(message);
        if (state === 'open') socket.send(text);
        else queue.push(text);
      }
    };
  }

  const channel = webViewChannel() || socketChannel();

  window.STHostTransport = {
    kind: channel ? channel.kind : 'none',
    available: !!channel,
    post(message) {
      if (!channel) throw new Error('No SuperTracker host is connected to this page.');
      channel.post(message);
    },
    onMessage,
    receive,
    socketURL
  };
})();
