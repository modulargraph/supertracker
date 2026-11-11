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

/* What the host answers to note recording, for tests whose subject is something else. The returned function
   takes a request and gives the result, or undefined when the request is none of these:
   - transport.recordArm confirms the arm;
   - a captured note edge (note.on / note.off with captureInput) is acknowledged as the audio thread does:
     stopped, or while `state.playing` at the pattern and row of `state.status` with a recording clock.
   `clock()` gives the engine clock of the next acknowledgement in beats (default: a beat per edge). */
function noteRecordingHost(window, { clock } = {}) {
  let requestID = 0, beats = 0;
  return ({ command, payload = {} }) => {
    if (command === 'transport.recordArm') return { armed: payload.armed === true };
    if ((command !== 'note.on' && command !== 'note.off') || !payload.captureInput) return undefined;
    const state = window.ST.state, playing = !!state.playing, status = state.status || {};
    const acknowledgement = { inputOwner: payload.inputOwner, requestID: String(++requestID), status: 'applied', applied: true, playing,
      recording: true, on: command === 'note.on', pattern: status.pattern ?? window.ST.pattern().id, row: status.row ?? 0,
      sequence: status.sequence ?? 0, track: payload.track, instrument: payload.instrument, note: payload.note,
      velocity: payload.velocity ?? 0, applyNote: payload.applyNote !== false,
      recordingContext: window.STInputCapture.recordingContext() };
    if (playing) acknowledgement.recordingClock = { distanceBeats: clock ? clock() : beats++, runID: '1' };
    return { inputAcknowledgement: acknowledgement };
  };
}

module.exports = { hostStub, audioExport, exportDelivery, noteRecordingHost };
