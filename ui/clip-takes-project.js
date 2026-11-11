/* Non-destructive clip alternatives. Native audio continues to read the selected
 * clip's ordinary path/sourceOffset; inactive takes are project media only. */
(() => {
  'use strict';
  const { clone } = window.STUtil;
  const fail = message => { throw new Error(message); };
  const finite = value => typeof value === 'number' && Number.isFinite(value);
  const pathOf = clip => clip.path || clip.samplePath || '';
  const maxTakes = 32;
  function regionSeconds(clip, secondsPerBeat) {
    if (!finite(clip.durationBeats) || clip.durationBeats <= 0) fail('The clip has an invalid duration.');
    const duration = typeof secondsPerBeat === 'function'
        ? secondsPerBeat(Number(clip.startBeat || 0), clip.durationBeats)
        : clip.durationBeats * secondsPerBeat;
    if (!finite(duration) || duration <= 0) fail('The clip has an invalid duration or tempo.');
    return duration;
  }
  function read(clip) {
    if (!clip || !pathOf(clip)) fail('Select an audio clip with a source file.');
    let set;
    if (clip.takeSet == null) {
      set = {
        activeID: `original-${clip.id}`,
        takes: [{ id: `original-${clip.id}`, name: clip.name || 'Original', path: pathOf(clip), sourceOffset: Number(clip.sourceOffset || 0) }]
      };
      if (finite(clip.sourceDuration) && clip.sourceDuration > 0) set.takes[0].sourceDuration = clip.sourceDuration;
    } else {
      set = clone(clip.takeSet);
      if (!Array.isArray(set.takes)
          || !set.takes.length
          || set.takes.length > maxTakes) fail('This clip has an invalid take list.');
      const ids = new Set();
      for (const take of set.takes) {
        if (!take
            || typeof take.id !== 'string'
            || !take.id
            || ids.has(take.id)
            || typeof take.path !== 'string'
            || !take.path
            || !finite(take.sourceOffset)
            || take.sourceOffset < 0
            || take.sourceDuration != null && (!finite(take.sourceDuration) || take.sourceDuration <= 0)) fail('This clip has invalid take data.');
        ids.add(take.id);
      }
      if (!ids.has(set.activeID)) fail('The selected take is missing from this clip.');
    }
    // Existing clip tools and portable media relinking own these canonical fields.
    // Capture their current values before retaining or switching an alternative.
    const active = set.takes.find(take => take.id === set.activeID);
    active.path = pathOf(clip); active.sourceOffset = Number(clip.sourceOffset || 0);
    if (!finite(active.sourceOffset) || active.sourceOffset < 0) fail('The clip has an invalid source offset.');
    return set;
  }
  function fits(clip, take, secondsPerBeat) {
    const length = regionSeconds(clip, secondsPerBeat);
    return take.sourceDuration == null || take.sourceOffset + length <= take.sourceDuration + 1e-7;
  }
  function project(clip, set) {
    const result = clone(clip), active = set.takes.find(take => take.id === set.activeID);
    if (!active) fail('Choose an existing take.');
    result.takeSet = set; result.path = active.path; result.sourceOffset = active.sourceOffset;
    // Avoid retaining an old alternate path on projects imported with samplePath.
    if ('samplePath' in result) result.samplePath = active.path;
    if (active.sourceDuration != null) result.sourceDuration = active.sourceDuration; else delete result.sourceDuration;
    return result;
  }
  function activate(clip, id, secondsPerBeat) {
    const set = read(clip), take = set.takes.find(item => item.id === id);
    if (!take) fail('The chosen take no longer exists.');
    if (!fits(clip, take, secondsPerBeat)) fail('This take is too short for the current region. Trim the region or adjust its source offset first.');
    set.activeID = id; return project(clip, set);
  }
  function add(clip, source, secondsPerBeat, makeID) {
    const set = read(clip);
    if (finite(source.currentSourceDuration)
        && source.currentSourceDuration > 0) set.takes.find(take => take.id === set.activeID).sourceDuration = source.currentSourceDuration;
    if (set.takes.length >= maxTakes) fail(`A clip can retain up to ${maxTakes} takes. Remove an unused alternative first.`);
    const take = {
      id: makeID(),
      name: String(source.name || `Take ${set.takes.length + 1}`).slice(0, 160),
      path: source.path,
      sourceOffset: source.sourceOffset ?? 0,
      sourceDuration: source.sourceDuration
    };
    if (typeof take.id !== 'string' || !take.id || set.takes.some(item => item.id === take.id)) fail('A unique take identity could not be created.');
    if (typeof take.path !== 'string'
        || !take.path
        || !finite(take.sourceOffset)
        || take.sourceOffset < 0
        || !finite(take.sourceDuration)
        || take.sourceDuration <= 0) fail('The alternate file must contain readable audio and a valid source offset.');
    if (!fits(clip, take, secondsPerBeat)) fail(`The alternate needs at least ${(take.sourceOffset + regionSeconds(clip, secondsPerBeat)).toFixed(2)} seconds to cover this region. `
        + `The source file has been preserved.`);
    set.takes.push(take); set.activeID = take.id; return project(clip, set);
  }
  function rename(clip, id, name) {
    const set = read(clip), take = set.takes.find(item => item.id === id);
    if (!take) fail('The chosen take no longer exists.');
    if (typeof name !== 'string' || !name.trim()) fail('Give the take a name.');
    take.name = name.trim().slice(0, 160); return project(clip, set);
  }
  function remove(clip, id) {
    const set = read(clip);
    if (id === set.activeID) fail('Choose another take before removing this alternative.');
    if (!set.takes.some(item => item.id === id)) fail('The chosen take no longer exists.');
    set.takes = set.takes.filter(item => item.id !== id); return project(clip, set);
  }
  function retainEdits(clip) {
    if (!clip.takeSet) return clip;
    return project(clip, read(clip));
  }
  function split(clip, point, secondsPerBeat, makeID) {
    const length = regionSeconds(clip, secondsPerBeat);
    if (!finite(point) || point <= .001 || point >= clip.durationBeats - .001) fail('Place the edit cursor inside the selected clip to split it.');
    const left = clone(retainEdits(clip)), right = clone(left), offset = regionSeconds({ ...clip, durationBeats: point }, secondsPerBeat);
    right.id = makeID(); right.startBeat = Number(clip.startBeat || 0) + point; right.durationBeats = clip.durationBeats - point;
    right.sourceOffset = Number(clip.sourceOffset || 0) + offset; right.fadeIn = 0;
    left.durationBeats = point; left.fadeOut = 0;
    left.fadeIn = Math.min(Number(left.fadeIn || 0), offset); right.fadeOut = Math.min(Number(right.fadeOut || 0), length - offset);
    if (right.takeSet) for (const take of right.takeSet.takes) take.sourceOffset += offset;
    return [left, right];
  }
  window.STClipTakesProject = { maxTakes, read, fits, activate, add, rename, remove, retainEdits, split, clone };
})();
