/* Fine control for continuous parameters.
   - Perceptual slider curves: a slider can move in "position" space (0..1) while the stored value
     follows an offset-logarithmic curve, so the low end of a rate, time or depth gets most of the travel.
   - Magnitude-relative steps: arrows change the second significant digit (2.5 → 2.6, 0.05 → 0.051,
     25% → 26%), ⌥ the third (1.25 → 1.26), ⇧ the first (2.5 → 3).
   - Fine drags on any range input: ⌥-drag moves at 1/10 speed, ⇧-drag at 1/4, ⌥⇧-drag at 1/50.
   Controls opt in with data-fine-* attributes, which the automation recorder also reads, so the stored
   value, automation lanes and MIDI control keep using engine units. Nothing here changes stored values. */
(() => {
  'use strict';
  const num = (value, fallback = 0) => { const n = typeof value === 'string' && value.trim() === '' ? NaN : Number(value); return Number.isFinite(n) ? n : fallback; };
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const clean = value => { const n = Number(Number(value).toPrecision(12)); return Object.is(n, -0) || Math.abs(n) < 1e-12 ? 0 : n; };

  // ---- Curves -----------------------------------------------------------------------------
  // value = min + knee · ((1 + span / knee)^position − 1). Smaller knees give the low end more travel.
  function toPosition(value, spec) {
    const min = num(spec.min), max = num(spec.max), knee = num(spec.knee), span = max - min;
    if (!(span > 0)) return 0;
    const x = clamp(num(value, min), min, max) - min;
    return knee > 0 ? clamp(Math.log1p(x / knee) / Math.log1p(span / knee), 0, 1) : x / span;
  }
  function fromPosition(position, spec) {
    const min = num(spec.min), max = num(spec.max), knee = num(spec.knee), span = max - min, p = clamp(num(position), 0, 1);
    if (!(span > 0) || p <= 0) return min;
    if (p >= 1) return max;
    return clamp(min + (knee > 0 ? knee * Math.expm1(p * Math.log1p(span / knee)) : p * span), min, max);
  }

  // ---- Steps ------------------------------------------------------------------------------
  function decadeOf(magnitude) {
    let decade = Math.floor(Math.log10(magnitude));
    if (10 ** (decade + 1) <= magnitude * (1 + 1e-12)) decade++;
    return decade;
  }
  /** Step size near `value` when moving in `direction`: a tenth of the value's decade (second significant
      digit), ⌥ (fine) a hundredth, ⇧ (coarse) the decade itself. Below `floor` the floor's decade applies.
      Moving toward zero uses the decade below, so 1 → 0.99 and ⌥ 1 → 0.999. */
  function stepSize(value, direction = 1, { fine = false, coarse = false } = {}, spec = {}) {
    const floor = num(spec.floor) > 0 ? num(spec.floor) : 1e-3, v = num(value);
    const towardZero = (direction < 0 && v > 0) || (direction > 0 && v < 0);
    const magnitude = Math.max(Math.abs(v) * (towardZero ? 1 - 1e-9 : 1), floor);
    return 10 ** (decadeOf(magnitude) - (fine ? 2 : coarse ? 0 : 1));
  }
  /** The next value on the step grid in `direction`, clamped to the spec's range. */
  function stepValue(value, direction, modifiers = {}, spec = {}) {
    const min = num(spec.min, -Infinity), max = num(spec.max, Infinity), v = clamp(num(value, Number.isFinite(min) ? min : 0), min, max);
    const dir = direction < 0 ? -1 : 1, step = stepSize(v, dir, modifiers, spec), ratio = v / step;
    const next = dir > 0 ? (Math.floor(ratio + 1e-9) + 1) * step : (Math.ceil(ratio - 1e-9) - 1) * step;
    return clamp(clean(next), min, max);
  }
  /** Rounds a dragged value to readable precision: three significant digits, never finer than floor/10. */
  function quantize(value, spec = {}) {
    const v = num(value), min = num(spec.min, -Infinity), max = num(spec.max, Infinity);
    if (v === 0) return clamp(0, min, max);
    const step = stepSize(v, Math.sign(v), { fine: true }, spec);
    return clamp(clean(Math.round(v / step) * step), min, max);
  }
  /** Compact number with up to `digits` significant digits (0.05, 1.25, 12.5, 0.0512). */
  function sig(value, digits = 4) { const v = num(value); return String(Math.abs(v) < 1e-9 ? 0 : clean(Number(v.toPrecision(digits)))); }

  // ---- Element contract (shared with the automation recorder) --------------------------------
  // data-fine-min / -max / -floor: lane (engine) units. data-fine-scale: element = lane × scale.
  // data-fine-knee: the element is a curved slider whose value is a 0..1 position.
  const has = el => el?.dataset?.fineMin !== undefined && el?.dataset?.fineMax !== undefined;
  const curved = el => has(el) && num(el.dataset.fineKnee) > 0;
  function specOf(el) {
    if (!has(el)) return null;
    const d = el.dataset;
    return { min: num(d.fineMin), max: num(d.fineMax), floor: num(d.fineFloor, 1e-3), knee: num(d.fineKnee), scale: num(d.fineScale, 1) || 1 };
  }
  function laneFromElement(el, raw = el?.value) {
    const spec = specOf(el); if (!spec) return num(raw, NaN);
    const v = num(raw, NaN); if (!Number.isFinite(v)) return NaN;
    return curved(el) ? quantize(fromPosition(v, spec), spec) : clamp(v / spec.scale, spec.min, spec.max);
  }
  function elementFromLane(el, value) {
    const spec = specOf(el); if (!spec) return String(value);
    const v = clamp(num(value, spec.min), spec.min, spec.max);
    return curved(el) ? String(clean(toPosition(v, spec))) : sig(v * spec.scale, 8);
  }
  /** Steps an opted-in element by one key press; returns the new lane value (element updated). */
  function stepElement(el, direction, modifiers) {
    const spec = specOf(el); if (!spec) return NaN;
    const current = laneFromElement(el);
    const next = stepValue(Number.isFinite(current) ? current : spec.min, direction, modifiers, spec);
    el.value = elementFromLane(el, next);
    return next;
  }

  // Plug-in parameters remain normalized, including Audio Units whose raw range is not 0..1.
  // stepCount is the native number of intervals (a two-state switch has one).
  function parameterSteps(info) {
    const count = Number(info?.stepCount ?? info?.dataset?.parameterSteps);
    return Number.isSafeInteger(count) && count > 0 ? count : 0;
  }
  function parameterStep(info) { const count = parameterSteps(info); return count ? 1 / count : .001; }
  function snapParameter(value, info) {
    const v = clamp(num(value), 0, 1), count = parameterSteps(info);
    return count ? Math.round(v * count) / count : v;
  }
  function stepParameter(value, direction, {fine = false, coarse = false} = {}, info) {
    const v = clamp(num(value), 0, 1), count = parameterSteps(info), dir = direction < 0 ? -1 : 1;
    if (count) return clamp(Math.round(v * count) + dir * (coarse ? Math.max(1, Math.round(count / 10)) : 1), 0, count) / count;
    return clamp(clean(v + dir * (coarse ? .1 : fine ? .001 : .01)), 0, 1);
  }

  // ---- Fine drags --------------------------------------------------------------------------
  const THUMB = 12;
  const speed = event => event.altKey ? (event.shiftKey ? .02 : .1) : event.shiftKey ? .25 : 1;
  const fire = (el, type) => el.dispatchEvent(new Event(type, { bubbles: true }));
  /** Takes over mouse drags on a range input so ⌥ / ⇧ (held at any time) slow it down. A plain click
      still jumps to the pointer and plain drags track it; touch keeps the native behaviour. Every move
      dispatches the usual input event and the release one change event, so existing handlers apply. */
  function bindDrag(el) {
    if (!el || el.type !== 'range' || el.dataset.fineDrag === 'true' || typeof el.addEventListener !== 'function') return;
    el.dataset.fineDrag = 'true';
    let drag = null;
    const limits = () => { const min = num(el.min, 0), max = num(el.max, 100); return { min, max: max > min ? max : min + 1 }; };
    const write = value => {
      const { min, max } = limits(), step = num(el.step, NaN);
      let next = clamp(value, min, max);
      if (step > 0) next = clamp(clean(min + Math.round((next - min) / step) * step), min, max);
      if (num(el.value, NaN) === next) return;
      el.value = String(next); drag.changed = true; fire(el, 'input');
    };
    const finish = event => {
      if (!drag || (event?.pointerId !== undefined && event.pointerId !== drag.id)) return;
      const changed = drag.changed; drag = null;
      try { el.releasePointerCapture?.(event.pointerId); } catch {}
      if (changed && el.isConnected !== false) fire(el, 'change');
    };
    el.addEventListener('pointerdown', event => {
      if (el.disabled || event.button > 0 || event.pointerType === 'touch') return;
      event.preventDefault();
      el.focus?.({ preventScroll: true });
      const rect = el.getBoundingClientRect?.() || { left: 0, width: 100 }, usable = Math.max(1, rect.width - THUMB), { min, max } = limits();
      const current = clamp(num(el.value, min), min, max), thumb = rect.left + THUMB / 2 + (current - min) / (max - min) * usable;
      drag = { id: event.pointerId, x: event.clientX, value: current, usable, changed: false };
      try { el.setPointerCapture?.(event.pointerId); } catch {}
      // Native behaviour: clicking the track jumps; grabbing the thumb (or a modified press) does not.
      if (speed(event) === 1 && Math.abs(event.clientX - thumb) > THUMB / 2) { drag.value = min + clamp((event.clientX - rect.left - THUMB / 2) / usable, 0, 1) * (max - min); write(drag.value); }
    });
    el.addEventListener('pointermove', event => {
      if (!drag || event.pointerId !== drag.id) return;
      const { min, max } = limits(), dx = event.clientX - drag.x; drag.x = event.clientX;
      if (!dx) return;
      drag.value = clamp(drag.value + dx / drag.usable * (max - min) * speed(event), min, max);
      write(drag.value);
    });
    el.addEventListener('pointerup', finish);
    el.addEventListener('pointercancel', finish);
    el.addEventListener('lostpointercapture', finish);
    // Belt and braces for engines that still send compatibility mouse events after a cancelled pointerdown.
    el.addEventListener('mousedown', event => { if (drag) event.preventDefault(); });
  }

  window.STFine = { toPosition, fromPosition, stepSize, stepValue, quantize, sig, has, curved, specOf, laneFromElement, elementFromLane, stepElement, bindDrag, speed, parameterSteps, parameterStep, snapParameter, stepParameter };
})();
