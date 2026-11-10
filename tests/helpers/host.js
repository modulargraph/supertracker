'use strict';
// A stand-in for ui/host-transport.js: the test plays the host.
//
//   window.STHostTransport = hostStub({ post(message) { ... } })
//       post() receives every request object the page sends ({id, command, payload}).
//   window.STHostTransport.deliver(message)
//       hands the page a reply ({id, ok, result | error}) or an event ({type, payload}),
//       synchronously, as the host's channel does.
//
// host-transport.js leaves a transport that is already installed alone, so a test may load every
// script of index.html after installing this one. Tests of the real module: host_transport_tests.js.
function hostStub(channel = {}) {
  const handlers = new Set();
  const deliver = message => { for (const handler of [...handlers]) handler(message); };
  return {
    kind: 'test',
    available: true,
    post: typeof channel.post === 'function' ? channel.post.bind(channel) : () => {},
    onMessage(handler) { handlers.add(handler); return () => handlers.delete(handler); },
    receive: deliver,
    deliver
  };
}

module.exports = { hostStub };
