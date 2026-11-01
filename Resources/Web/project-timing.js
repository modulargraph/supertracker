/* Continuous musical-time queries. Audio sample quantization stays native. */
(function (root) {
  'use strict';
  const MAX_BEAT = 1000000, MAX_EVENTS = 1024;
  const own = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
  const fail = message => { throw new Error('Tempo map: ' + message); };
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const numeric = (value, label) => {
    if (typeof value !== 'number' || !Number.isFinite(value)) fail(label + ' must be a finite number.');
    return value;
  };
  const position = value => {
    numeric(value, 'Quarter-beat position');
    if (value < 0 || value > MAX_BEAT) fail('Quarter-beat position must be from 0–1000000.');
    return value;
  };
  function validate(project) {
    if (!object(project)) fail('Expected a project object.');
    if (!own(project, 'timing')) return null;
    const timing = project.timing;
    if (!object(timing)) fail('Expected a timing object.');
    if (numeric(timing.version, 'Version') !== 1) fail('Unsupported timing version.');
    const result = { version: 1 };
    for (const kind of ['tempos', 'meters']) {
      const rows = timing[kind], ids = new Set();
      if (!Array.isArray(rows) || rows.length < 1 || rows.length > MAX_EVENTS) fail('Provide 1–1024 tempo events and 1–1024 meter events.');
      result[kind] = rows.map((row, index) => {
        const label = (kind === 'tempos' ? 'Tempo' : 'Meter') + ' event ' + (index + 1);
        if (!object(row)) fail(label + ' must be an object.');
        if (typeof row.id !== 'string' || !row.id.trim() || new TextEncoder().encode(row.id).length > 256) fail(label + ' needs a nonempty ID of at most 256 bytes.');
        if (ids.has(row.id)) fail(label + ' repeats an event ID.');
        ids.add(row.id);
        const beat = numeric(row.beat, label + ' beat');
        if (beat < 0 || beat > MAX_BEAT || (index === 0 ? beat !== 0 : beat <= rows[index - 1].beat)) fail('Each event list must start at beat zero and increase without duplicate beats.');
        if (kind === 'tempos') {
          const bpm = numeric(row.bpm, label + ' BPM');
          if (bpm < 20 || bpm > 400) fail(label + ' needs a tempo from 20–400 BPM.');
          return { id: row.id, beat: beat === 0 ? 0 : beat, bpm };
        }
        const numerator = numeric(row.numerator, label + ' numerator'), denominator = numeric(row.denominator, label + ' denominator');
        if (!Number.isInteger(numerator) || numerator < 1 || numerator > 16 || ![2, 4, 8, 16].includes(denominator)) fail(label + ' needs meter 1–16 over 2, 4, 8 or 16.');
        return { id: row.id, beat: beat === 0 ? 0 : beat, numerator, denominator };
      });
    }
    return result;
  }
  // Match AudioProject's NSNumber fallback and the legacy engine clamps. In
  // particular, a missing map does not lower a valid legacy 999 BPM to 400.
  const legacyNumber = (value, fallback) => (typeof value === 'number' || typeof value === 'boolean') && Number.isFinite(Number(value)) ? Number(value) : fallback;
  function legacy(project) {
    const signature = object(project.timeSignature) ? project.timeSignature : {};
    const numerator = Math.max(1, Math.min(16, Math.trunc(legacyNumber(signature.numerator, 4))));
    const rawDenominator = Math.trunc(legacyNumber(signature.denominator, 4));
    return { bpm: Math.max(20, Math.min(999, legacyNumber(project.bpm, 120))), numerator, denominator: [2, 4, 8, 16].includes(rawDenominator) ? rawDenominator : 4 };
  }
  function seed(project, uid) {
    const existing = validate(project);
    if (existing) return existing;
    const first = legacy(project), makeID = typeof uid === 'function' ? uid : () => root.crypto?.randomUUID?.() || 'timing-' + Math.random().toString(36).slice(2);
    return { version: 1, tempos: [{ id: makeID(), beat: 0, bpm: first.bpm }], meters: [{ id: makeID(), beat: 0, numerator: first.numerator, denominator: first.denominator }] };
  }
  function indexAt(rows, value, field = 'beat') {
    let low = 0, high = rows.length;
    while (low < high) { const middle = (low + high) >>> 1; if (rows[middle][field] <= value) low = middle + 1; else high = middle; }
    return Math.max(0, low - 1);
  }
  const barStart = (segment, offset) => segment.beat + offset * segment.quartersPerBar;
  function barOffset(segment, beat) {
    let offset = Math.floor((beat - segment.beat) / segment.quartersPerBar);
    if (offset > 0 && beat < barStart(segment, offset)) offset--;
    if (beat >= barStart(segment, offset + 1)) offset++;
    return offset;
  }
  const floatBits = new DataView(new ArrayBuffer(8));
  function previousPositive(value) {
    floatBits.setFloat64(0, value); floatBits.setBigUint64(0, floatBits.getBigUint64(0) - 1n); return floatBits.getFloat64(0);
  }
  function create(project) {
    const timing = validate(project), fixed = timing ? null : legacy(project);
    const tempoRows = timing?.tempos || [{ id: 'legacy-tempo', beat: 0, bpm: fixed.bpm }];
    const meterRows = timing?.meters || [{ id: 'legacy-meter', beat: 0, numerator: fixed.numerator, denominator: fixed.denominator }];
    let seconds = 0, correction = 0;
    const tempos = tempoRows.map((row, index) => {
      if (index) {
        const previous = tempoRows[index - 1], adjusted = (row.beat - previous.beat) * 60 / previous.bpm - correction;
        const next = seconds + adjusted; correction = (next - seconds) - adjusted;
        if (!Number.isFinite(next) || next <= seconds) fail('Tempo boundaries are too close to distinguish in seconds.');
        seconds = next;
      }
      return Object.freeze({ ...row, seconds });
    });
    const meters = [];
    for (const row of meterRows) {
      const previous = meters.at(-1), offset = previous ? barOffset(previous, row.beat) : 0;
      const firstBar = previous ? previous.firstBar + offset + (row.beat === barStart(previous, offset) ? 0 : 1) : 1;
      meters.push(Object.freeze({ ...row, firstBar, quartersPerBar: row.numerator * 4 / row.denominator }));
    }
    const last = tempos.at(-1), maxSeconds = last.seconds + (MAX_BEAT - last.beat) * 60 / last.bpm;
    function secondsAtBeat(beat) {
      position(beat); if (beat === MAX_BEAT) return maxSeconds;
      const index = indexAt(tempos, beat), segment = tempos[index];
      // Keep the old fixed-tempo multiplication order for projects without maps.
      const value = timing ? segment.seconds + (beat - segment.beat) * (60 / segment.bpm) : beat * 60 / segment.bpm;
      return Math.min(value, tempos[index + 1]?.seconds ?? maxSeconds);
    }
    function beatAtSeconds(value) {
      numeric(value, 'Seconds'); if (value < 0 || value > maxSeconds) fail('Seconds are outside the supported map.');
      if (value === maxSeconds) return MAX_BEAT;
      const index = indexAt(tempos, value, 'seconds'), segment = tempos[index];
      if (value === segment.seconds) return segment.beat;
      return Math.min(segment.beat + (value - segment.seconds) * segment.bpm / 60, tempos[index + 1]?.beat ?? MAX_BEAT);
    }
    function tempoAtBeat(beat) { position(beat); return tempos[indexAt(tempos, beat)].bpm; }
    function meterAtBeat(beat) { position(beat); const value = meters[indexAt(meters, beat)]; return { numerator: value.numerator, denominator: value.denominator }; }
    function barAtBeat(beat) {
      position(beat); const index = indexAt(meters, beat), segment = meters[index], offset = barOffset(segment, beat);
      const start = barStart(segment, offset), nominalEnd = barStart(segment, offset + 1), end = Math.min(nominalEnd, meters[index + 1]?.beat ?? nominalEnd);
      let notatedBeat = 1 + (beat - start) * segment.denominator / 4;
      if (notatedBeat >= segment.numerator + 1) notatedBeat = previousPositive(segment.numerator + 1);
      if (notatedBeat > 1 && start + (notatedBeat - 1) * 4 / segment.denominator >= end) notatedBeat = previousPositive(notatedBeat);
      return { bar: segment.firstBar + offset, notatedBeat, numerator: segment.numerator, denominator: segment.denominator, quarterOffset: beat - start, barStart: start, barEnd: end, shortBar: end < nominalEnd };
    }
    function labelAtBeat(beat) {
      const at = barAtBeat(beat), value = Math.floor(at.notatedBeat * 1000) / 1000;
      return `Bar ${at.bar} · beat ${value} · ${at.numerator}/${at.denominator}`;
    }
    function duration(startBeat, endBeat) { if (endBeat < startBeat) fail('End beat precedes start beat.'); return secondsAtBeat(endBeat) - secondsAtBeat(startBeat); }
    function endingBeat(startBeat, durationSeconds) { numeric(durationSeconds, 'Audio duration'); if (durationSeconds < 0) fail('Audio duration must be nonnegative.'); return beatAtSeconds(secondsAtBeat(startBeat) + durationSeconds); }
    return Object.freeze({ mapped: timing !== null, tempos: Object.freeze(tempos), meters: Object.freeze(meters), maxSeconds, secondsAtBeat, beatAtSeconds, tempoAtBeat, meterAtBeat, barAtBeat, labelAtBeat, duration, endingBeat });
  }
  const api = { MAX_BEAT, MAX_EVENTS, validate, create, seed,
    secondsAtBeat: (project, beat) => create(project).secondsAtBeat(beat),
    beatAtSeconds: (project, seconds) => create(project).beatAtSeconds(seconds),
    durationFromBeats: (project, startBeat, durationBeats) => { numeric(durationBeats, 'Duration'); if (durationBeats < 0) fail('Duration must be nonnegative.'); return create(project).duration(startBeat, startBeat + durationBeats); },
    endingBeat: (project, startBeat, durationSeconds) => create(project).endingBeat(startBeat, durationSeconds) };
  root.STProjectTiming = Object.freeze(api);
  if (typeof module !== 'undefined' && module.exports) module.exports = root.STProjectTiming;
})(typeof window !== 'undefined' ? window : globalThis);
