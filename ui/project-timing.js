/* The project's tempo and meter: quarter beats, bars and seconds. A project has one tempo (bpm) and one meter
   (timeSignature) for its whole length. Audio sample quantization stays in the host. */
(function (root) {
  'use strict';
  const MAX_BEAT = 1000000;
  const fail = message => { throw new Error('Timing: ' + message); };
  const numeric = (value, label) => {
    if (typeof value !== 'number' || !Number.isFinite(value)) fail(label + ' must be a finite number.');
    return value;
  };
  const position = value => {
    numeric(value, 'Quarter-beat position');
    if (value < 0 || value > MAX_BEAT) fail('Quarter-beat position must be from 0–1000000.');
    return value;
  };
  const number = (value, fallback) => typeof value === 'number' && Number.isFinite(value) ? value : fallback;
  // The project format's ranges (st-project validates the same ones); a missing value takes the default.
  const tempo = project => Math.max(20, Math.min(400, number(project?.bpm, 120)));
  function meter(project) {
    const signature = project?.timeSignature && typeof project.timeSignature === 'object' ? project.timeSignature : {};
    const denominator = Math.trunc(number(signature.denominator, 4));
    return {
      numerator: Math.max(1, Math.min(16, Math.trunc(number(signature.numerator, 4)))),
      denominator: [2, 4, 8, 16].includes(denominator) ? denominator : 4
    };
  }
  const floatBits = new DataView(new ArrayBuffer(8));
  function previousPositive(value) {
    floatBits.setFloat64(0, value); floatBits.setBigUint64(0, floatBits.getBigUint64(0) - 1n); return floatBits.getFloat64(0);
  }
  // A snapshot of the project's tempo and meter: later edits of the project do not change it.
  function create(project) {
    const bpm = tempo(project), { numerator, denominator } = meter(project), quartersPerBar = numerator * 4 / denominator;
    const maxSeconds = MAX_BEAT * 60 / bpm;
    const secondsAtBeat = beat => Math.min(position(beat) * 60 / bpm, maxSeconds);
    function beatAtSeconds(value) {
      numeric(value, 'Seconds'); if (value < 0 || value > maxSeconds) fail('Seconds are outside the supported range.');
      return Math.min(value * bpm / 60, MAX_BEAT);
    }
    function barAtBeat(beat) {
      position(beat);
      // The division can land one bar off next to a bar line.
      let offset = Math.floor(beat / quartersPerBar);
      if (offset > 0 && beat < offset * quartersPerBar) offset--;
      if (beat >= (offset + 1) * quartersPerBar) offset++;
      const start = offset * quartersPerBar, end = (offset + 1) * quartersPerBar;
      let notatedBeat = 1 + (beat - start) * denominator / 4;
      if (notatedBeat >= numerator + 1) notatedBeat = previousPositive(numerator + 1);
      if (notatedBeat > 1 && start + (notatedBeat - 1) * 4 / denominator >= end) notatedBeat = previousPositive(notatedBeat);
      return { bar: offset + 1, notatedBeat, numerator, denominator, quarterOffset: beat - start, barStart: start, barEnd: end };
    }
    function duration(startBeat, endBeat) { if (endBeat < startBeat) fail('End beat precedes start beat.'); return secondsAtBeat(endBeat) - secondsAtBeat(startBeat); }
    function endingBeat(startBeat, durationSeconds) { numeric(durationSeconds, 'Audio duration'); if (durationSeconds < 0) fail('Audio duration must be nonnegative.'); return beatAtSeconds(secondsAtBeat(startBeat) + durationSeconds); }
    return Object.freeze({ bpm, numerator, denominator, quartersPerBar, maxSeconds, secondsAtBeat, beatAtSeconds, barAtBeat, duration, endingBeat });
  }
  const api = { MAX_BEAT, tempo, meter, create,
    durationFromBeats: (project, startBeat, durationBeats) => { numeric(durationBeats, 'Duration'); if (durationBeats < 0) fail('Duration must be nonnegative.'); return create(project).duration(startBeat, startBeat + durationBeats); },
    endingBeat: (project, startBeat, durationSeconds) => create(project).endingBeat(startBeat, durationSeconds) };
  root.STProjectTiming = Object.freeze(api);
  if (typeof module !== 'undefined' && module.exports) module.exports = root.STProjectTiming;
})(typeof window !== 'undefined' ? window : globalThis);
