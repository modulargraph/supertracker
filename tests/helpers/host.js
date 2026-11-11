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

// What the host says about audio export in its answer to routing.capabilities
// (st_samples::export::format::capabilities), and the delivery it confirms in an audio.export result.
const audioExport = () => ({ container: 'wav', sampleRate: 48000, sampleRates: [44100, 48000, 96000], defaultSampleRate: 48000,
  encodings: ['pcm16', 'pcm24', 'float32'], defaultEncoding: 'pcm24', dithers: ['none', 'tpdf'], defaultDither: 'tpdf' });
function exportDelivery(payload = {}) {
  const encoding = payload.encoding ?? 'pcm24';
  return { container: 'wav', encoding, sampleRate: payload.sampleRate ?? 48000, dither: encoding === 'float32' ? 'none' : payload.dither ?? 'tpdf' };
}

module.exports = { hostStub, audioExport, exportDelivery };
