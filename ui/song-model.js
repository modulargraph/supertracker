/* Song arrangement model: the engine plays `project.sequence`, a flat list of
   pattern ids. The arranger presents consecutive equal ids as one block with a
   repeat count, so every edit stays a plain sequence edit (export, MIDI export,
   clip placement and the engine need no new data). An optional section loop
   `project.songLoop = {start, end}` holds inclusive flat sequence indices; the
   native host applies it only while `loopSong` is on (export turns that off). */
(() => {
  'use strict';
  const { clone } = typeof window !== 'undefined' && window.STUtil ? window.STUtil : require('./util.js');
  const MAX_ENTRIES = 1024, MAX_REPEATS = 64;
  const COLORS = ['#d6a35c', '#7fb7a4', '#c9785d', '#8fa4d4', '#b48ecf', '#cfc06a', '#6fae6a', '#d07fa0', '#7a8a99'];
  const clampInt = (value, lo, hi) => Math.min(hi, Math.max(lo, Math.trunc(Number(value)) || 0));

  function blocks(sequence) {
    const result = [];
    sequence.forEach((id, index) => {
      const last = result[result.length - 1];
      if (last && last.pattern === id) last.count++;
      else result.push({ index: result.length, start: index, count: 1, pattern: id });
    });
    return result;
  }
  function blockAt(sequence, flat) {
    const list = blocks(sequence);
    const at = list.findIndex(block => flat >= block.start && flat < block.start + block.count);
    return at < 0 ? Math.max(0, list.length - 1) : at;
  }
  /* Valid section loop for a project, or null. */
  function loopRange(project) {
    const loop = project.songLoop, length = project.sequence.length;
    if (!loop || !Number.isInteger(loop.start) || !Number.isInteger(loop.end)) return null;
    if (loop.start < 0 || loop.end < loop.start || loop.start >= length) return null;
    return { start: loop.start, end: Math.min(loop.end, length - 1) };
  }
  /* Edits run on entries that carry their loop membership, so inserting,
     removing and moving blocks keeps the section on the same music. */
  function entries(project) {
    const loop = loopRange(project);
    return project.sequence.map((id, index) => ({ id, loop: !!loop && index >= loop.start && index <= loop.end }));
  }
  function commit(project, list) {
    if (!list.length) throw new Error('The song needs at least one pattern.');
    if (list.length > MAX_ENTRIES) throw new Error(`The song supports up to ${MAX_ENTRIES} sequence slots.`);
    project.sequence = list.map(entry => entry.id);
    const inside = list.map((entry, index) => entry.loop ? index : -1).filter(index => index >= 0);
    if (project.songLoop && inside.length) {
      // A section always covers whole blocks, also after edits merged runs.
      const list = blocks(project.sequence),
        first = list[blockAt(project.sequence, inside[0])],
        last = list[blockAt(project.sequence, inside[inside.length - 1])];
      project.songLoop = { ...project.songLoop, start: first.start, end: last.start + last.count - 1 };
    }
    else if (project.songLoop) clearLoop(project);
    return project;
  }
  function span(project, from, to = from) {
    const list = blocks(project.sequence), a = clampInt(Math.min(from, to), 0, list.length - 1), b = clampInt(Math.max(from, to), 0, list.length - 1);
    return { list, a, b, start: list[a].start, end: list[b].start + list[b].count };
  }
  function patternExists(project, id) { return project.patterns.some(pattern => pattern.id === id); }

  function setRepeats(project, block, count) {
    const { start, end } = span(project, block), all = entries(project), entry = all[start];
    const next = clampInt(count, 1, MAX_REPEATS);
    all.splice(start, end - start, ...Array.from({ length: next }, () => ({ ...entry })));
    commit(project, all);
    return blockAt(project.sequence, start);
  }
  function setPattern(project, block, id) {
    if (!patternExists(project, id)) throw new Error('That pattern does not exist.');
    const { start, end } = span(project, block), all = entries(project);
    for (let index = start; index < end; index++) all[index].id = id;
    commit(project, all);
    return blockAt(project.sequence, start);
  }
  /* Insert `count` slots of `id` after (or before) a block; returns the block index of the insert. */
  function insert(project, block, id, { before = false, count = 1 } = {}) {
    if (!patternExists(project, id)) throw new Error('That pattern does not exist.');
    const { start, end } = span(project, block), all = entries(project), at = before ? start : end;
    const inLoop = !!(all[at - 1]?.loop && all[at]?.loop);
    all.splice(at, 0, ...Array.from({ length: clampInt(count, 1, MAX_REPEATS) }, () => ({ id, loop: inLoop })));
    commit(project, all);
    return blockAt(project.sequence, at);
  }
  function remove(project, from, to = from) {
    const { start, end } = span(project, from, to), all = entries(project);
    if (end - start >= all.length) throw new Error('The song needs at least one pattern.');
    all.splice(start, end - start);
    commit(project, all);
    return Math.min(blocks(project.sequence).length - 1, blockAt(project.sequence, Math.min(start, project.sequence.length - 1)));
  }
  /* Move blocks a..b one block up (-1) or down (+1); returns the new first block index or -1. */
  function move(project, from, to, delta) {
    const { list, a, b, start, end } = span(project, from, to);
    const neighbor = delta < 0 ? list[a - 1] : list[b + 1];
    if (!neighbor) return -1;
    const all = entries(project), chunk = all.slice(start, end), other = all.slice(neighbor.start, neighbor.start + neighbor.count);
    if (delta < 0) all.splice(neighbor.start, other.length + chunk.length, ...chunk, ...other);
    else all.splice(start, chunk.length + other.length, ...other, ...chunk);
    commit(project, all);
    return blockAt(project.sequence, delta < 0 ? neighbor.start : start + other.length);
  }
  /* Copy blocks a..b right after b; returns the first block index of the copy. */
  function duplicate(project, from, to = from) {
    const { start, end } = span(project, from, to), all = entries(project);
    const copy = all.slice(start, end).map(entry => ({ ...entry, loop: !!(entry.loop && all[end]?.loop) }));
    all.splice(end, 0, ...copy);
    commit(project, all);
    return blockAt(project.sequence, end);
  }
  /* Drop every slot of a deleted pattern. The section loop stays on its remaining music,
     or is cleared (restoring Loop song) when nothing of it is left. */
  function removePattern(project, id) {
    const all = entries(project).filter(entry => entry.id !== id);
    if (!all.length) {
      const fallback = project.patterns.find(pattern => pattern.id !== id);
      all.push({ id: fallback ? fallback.id : 0, loop: false });
    }
    return commit(project, all);
  }
  function nextPatternId(project, id, delta) {
    const list = project.patterns, index = list.findIndex(pattern => pattern.id === id);
    if (!list.length) return id;
    return list[((index < 0 ? 0 : index) + delta % list.length + list.length) % list.length].id;
  }
  /* 1-based pattern number in the project's pattern list (what the arranger shows). */
  function patternNumber(project, id) { return project.patterns.findIndex(pattern => pattern.id === id) + 1; }
  function patternByNumber(project, number) { return project.patterns[number - 1] || null; }
  function freePatternId(project) {
    for (let id = 0; id < 256; id++) if (!project.patterns.some(pattern => pattern.id === id)) return id;
    return -1;
  }
  function blankPattern(project, length = 64) {
    const id = freePatternId(project);
    if (id < 0) throw new Error('This session supports up to 256 patterns.');
    return { id, name: 'Pattern '
        + String(id + 1).padStart(2, '0'), length, steps: project.tracks.map(() => Array(length).fill(null)), automation: [] };
  }
  function clonePattern(project, id, uid = () => Math.random().toString(36).slice(2)) {
    const source = project.patterns.find(pattern => pattern.id === id);
    if (!source) throw new Error('That pattern does not exist.');
    const free = freePatternId(project);
    if (free < 0) throw new Error('This session supports up to 256 patterns.');
    const copy = clone(source);
    copy.id = free; copy.name = `${source.name || 'Pattern'} copy`;
    for (const lane of copy.automation) lane.id = uid();
    return copy;
  }
  function setLoop(project, from, to = from) {
    const { start, end } = span(project, from, to);
    const restore = project.songLoop ? project.songLoop.restoreLoopSong : !!project.loopSong;
    project.songLoop = { start, end: end - 1, restoreLoopSong: !!restore };
    project.loopSong = true;
    return project.songLoop;
  }
  function clearLoop(project) {
    if (!project.songLoop) return;
    if (typeof project.songLoop.restoreLoopSong === 'boolean') project.loopSong = project.songLoop.restoreLoopSong;
    delete project.songLoop;
  }
  function cycleColor(project, id, delta = 1) {
    const pattern = project.patterns.find(item => item.id === id);
    if (!pattern) return null;
    const index = COLORS.indexOf(pattern.color);
    pattern.color = COLORS[((index < 0 ? (delta > 0 ? -1 : 0) : index) + delta + COLORS.length) % COLORS.length];
    return pattern.color;
  }
  // Callers write the colour into style attributes: only a hex colour (what cycleColor stores) is taken from the file.
  function patternColor(project, id) {
    const pattern = project.patterns.find(item => item.id === id);
    if (/^#[0-9a-f]{3,8}$/i.test(pattern?.color || '')) return pattern.color;
    const number = patternNumber(project, id);
    return COLORS[(Math.max(1, number) - 1) % COLORS.length];
  }
  /* Quarter-note beat at which a flat sequence index starts (patterns use linesPerBeat). */
  function startBeat(project, flat) {
    const lpb = project.linesPerBeat || 4;
    return project.sequence.slice(0, flat).reduce((sum, id) => sum
        + (project.patterns.find(pattern => pattern.id === id)?.length || 0) / lpb,
        0);
  }
  /* Ordered list of {sequence, pattern, row} that playback visits (one song pass). */
  function playbackOrder(project) {
    const order = [];
    project.sequence.forEach((id, sequence) => {
      const pattern = project.patterns.find(item => item.id === id);
      for (let row = 0; row < (pattern?.length || 0); row++) order.push({ sequence, pattern: id, row });
    });
    return order;
  }

  const api = {
    MAX_REPEATS,
    COLORS,
    blocks,
    blockAt,
    loopRange,
    setRepeats,
    setPattern,
    insert,
    remove,
    removePattern,
    move,
    duplicate,
    nextPatternId,
    patternNumber,
    patternByNumber,
    freePatternId,
    blankPattern,
    clonePattern,
    setLoop,
    clearLoop,
    cycleColor,
    patternColor,
    startBeat,
    playbackOrder
  };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STSongModel = api;
})();
