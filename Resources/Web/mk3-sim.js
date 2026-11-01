/* Maschine MK3 software simulator: a clickable MK3 that feeds synthetic 'mk3.input' events into
   STMK3.event and renders the same LED payload and display lists the native driver receives.
   Open it from the command palette ("Maschine MK3 simulator") or with ?mk3sim in the URL.
   Mouse: press = hold, release = let go. Shift-click (or the sticky SHIFT button) adds Shift; Alt-click or
   right-click latches a button down until clicked again. Encoders: drag vertically or scroll; hovering
   touches them. Pads: the vertical click position sets velocity (top = loud). */
(() => {
  'use strict';
  const MK3 = () => window.STMK3;
  const sim = { root: null, unsubscribe: null, sticky: new Set(), shiftSticky: false, scale: 1, lastFrame: null,
    heldButtons: new Set(), heldPads: new Set(), heldTouches: new Set(), pedalDown: false };
  // Release only inputs started in this panel. Native controller inputs are never cancelled here.
  function send(payload) {
    if (!sim.root) return;
    const update = (set, key, down) => down ? set.add(key) : set.delete(key);
    if (payload.type === 'button') { if (!payload.pressed && !sim.heldButtons.has(payload.name)) return; update(sim.heldButtons, payload.name, payload.pressed); }
    else if (payload.type === 'pad') { if (payload.phase !== 'down' && !sim.heldPads.has(payload.pad)) return; if (payload.phase !== 'pressure') update(sim.heldPads, payload.pad, payload.phase === 'down'); }
    else if (payload.type === 'touch') { if (!payload.touched && !sim.heldTouches.has(payload.target)) return; update(sim.heldTouches, payload.target, payload.touched); }
    else if (payload.type === 'pedal') { if (!payload.down && !sim.pedalDown) return; sim.pedalDown = !!payload.down; }
    return MK3()?.event('mk3.input', payload);
  }
  const LABELS = { channel: 'CHANNEL', plugin: 'PLUG-IN', arranger: 'ARRANGER', mixer: 'MIXER', browser: 'BROWSER', sampling: 'SAMPLING', arrowLeft: '◄', arrowRight: '►', file: 'FILE', settings: 'SETTINGS', auto: 'AUTO', macro: 'MACRO',
    notes: 'NOTES', volume: 'VOLUME', swing: 'SWING', tempo: 'TEMPO', noteRepeat: 'NOTE REPEAT', lock: 'LOCK', pitch: 'PITCH', mod: 'MOD', perform: 'PERFORM', padMode: 'PAD MODE', keyboard: 'KEYBOARD', chords: 'CHORDS', step: 'STEP',
    fixedVel: 'FIXED VEL', scene: 'SCENE', pattern: 'PATTERN', events: 'EVENTS', variation: 'VARIATION', duplicate: 'DUPLICATE', select: 'SELECT', solo: 'SOLO', mute: 'MUTE', restart: 'RESTART', erase: 'ERASE', tap: 'TAP', follow: 'FOLLOW',
    play: '▶ PLAY', rec: '● REC', stop: '■ STOP', shift: 'SHIFT', groupA: 'A', groupB: 'B', groupC: 'C', groupD: 'D', groupE: 'E', groupF: 'F', groupG: 'G', groupH: 'H' };
  const SUB = { channel: 'MIDI', plugin: 'Instance', browser: '+Plug-in', file: 'Save', macro: 'Set', noteRepeat: 'Arp', lock: 'Exit', fixedVel: '16 Vel', scene: 'Section', perform: 'FX Select', variation: 'Navigate', duplicate: 'Double', mute: 'Choke', restart: 'Loop', erase: 'Replace', tap: 'Metro', follow: 'Grid', rec: 'Count-in', stop: 'Panic' };
  const el = (tag, className, text) => { const node = document.createElement(tag); if (className) node.className = className; if (text != null) node.textContent = text; return node; };

  function buttonNode(name, extra = '') {
    const node = el('button', `mk3-btn ${extra}`); node.type = 'button'; node.dataset.mk3Button = name; node.title = name;
    node.append(el('span', 'mk3-btn-label', LABELS[name] || name.replace('display', '')));
    if (SUB[name]) node.append(el('span', 'mk3-btn-sub', SUB[name]));
    return node;
  }
  function build() {
    const root = el('section', 'mk3-sim'); root.setAttribute('aria-label', 'Maschine MK3 simulator'); root.tabIndex = -1;
    const head = el('header', 'mk3-sim-head');
    head.append(el('strong', '', 'Maschine MK3 simulator'));
    const status = el('span', 'mk3-sim-status', ''); status.id = 'mk3-sim-status'; head.append(status);
    const zoom = el('button', 'quiet', '±'); zoom.title = 'Scale simulator'; zoom.id = 'mk3-sim-zoom'; head.append(zoom);
    const close = el('button', 'quiet', '×'); close.title = 'Close simulator'; close.id = 'mk3-sim-close'; head.append(close);
    root.append(head);
    const body = el('div', 'mk3-sim-body');
    // Top half: mode column, display buttons, screens, encoders, 4-D encoder block.
    const top = el('div', 'mk3-top');
    const modes = el('div', 'mk3-modes');
    for (const name of ['channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'arrowLeft', 'arrowRight', 'file', 'settings', 'auto', 'macro']) modes.append(buttonNode(name, 'small'));
    top.append(modes);
    const center = el('div', 'mk3-center');
    const displays = el('div', 'mk3-displays'); for (let i = 1; i <= 8; i++) displays.append(buttonNode('display' + i, 'display'));
    const screens = el('div', 'mk3-screens');
    for (const side of ['left', 'right']) { const canvas = el('canvas', 'mk3-screen'); canvas.width = 480; canvas.height = 272; canvas.dataset.mk3Screen = side; screens.append(canvas); }
    const encoders = el('div', 'mk3-encoders');
    for (let i = 0; i < 8; i++) { const knob = el('div', 'mk3-encoder'); knob.dataset.mk3Encoder = String(i); knob.title = `Encoder ${i + 1}: drag or scroll; hover touches`; knob.append(el('i')); encoders.append(knob); }
    center.append(displays, screens, encoders);
    top.append(center);
    const side = el('div', 'mk3-side');
    // Encoder mode buttons (VOLUME, SWING, TEMPO) beside the 4-D encoder; NOTES is the fourth touch-strip mode button.
    const perf = el('div', 'mk3-grid2'); for (const name of ['volume', 'swing', 'tempo']) perf.append(buttonNode(name, 'small'));
    const main = el('div', 'mk3-main');
    main.append(Object.assign(buttonNode('encoderUp', 'tilt up'), { textContent: '▲' }), Object.assign(buttonNode('encoderLeft', 'tilt left'), { textContent: '◀' }));
    const knob = el('div', 'mk3-main-knob'); knob.dataset.mk3Main = '1'; knob.title = '4-D encoder: scroll to turn, click to push, hover touches'; knob.append(el('i')); main.append(knob);
    main.append(Object.assign(buttonNode('encoderRight', 'tilt right'), { textContent: '▶' }), Object.assign(buttonNode('encoderDown', 'tilt down'), { textContent: '▼' }));
    const repeat = el('div', 'mk3-grid2'); for (const name of ['noteRepeat', 'lock']) repeat.append(buttonNode(name, 'small'));
    side.append(perf, main, repeat);
    top.append(side);
    body.append(top);
    // Bottom half: strip modes + strip, groups, transport; pad section.
    const bottom = el('div', 'mk3-bottom');
    const left = el('div', 'mk3-bleft');
    const stripModes = el('div', 'mk3-row'); for (const name of ['pitch', 'mod', 'perform', 'notes']) stripModes.append(buttonNode(name, 'small'));
    const strip = el('div', 'mk3-strip'); strip.dataset.mk3Strip = '1'; strip.title = 'Touch strip: drag'; for (let i = 0; i < 25; i++) strip.append(el('i'));
    const groups = el('div', 'mk3-groups'); for (const name of ['groupA', 'groupB', 'groupC', 'groupD', 'groupE', 'groupF', 'groupG', 'groupH']) { const group = buttonNode(name, 'group'); group.append(el('span', 'mk3-group-name', '')); groups.append(group); }
    const transport = el('div', 'mk3-transport'); for (const name of ['restart', 'erase', 'tap', 'follow', 'play', 'rec', 'stop', 'shift']) transport.append(buttonNode(name, 'small'));
    left.append(stripModes, strip, groups, transport);
    const right = el('div', 'mk3-bright');
    const padTop = el('div', 'mk3-padtop'); for (const name of ['scene', 'pattern', 'events', 'variation', 'duplicate', 'select', 'solo', 'mute']) padTop.append(buttonNode(name, 'small'));
    const padArea = el('div', 'mk3-padarea');
    const padModes = el('div', 'mk3-padmodes'); for (const name of ['fixedVel', 'padMode', 'keyboard', 'chords', 'step']) padModes.append(buttonNode(name, 'small'));
    const pads = el('div', 'mk3-pads');
    for (let row = 3; row >= 0; row--) for (let col = 0; col < 4; col++) { const n = row * 4 + col + 1; const pad = el('div', 'mk3-pad'); pad.dataset.mk3Pad = String(n); pad.append(el('span', 'mk3-pad-number', String(n)), el('span', 'mk3-pad-name', '')); pads.append(pad); }
    padArea.append(padModes, pads);
    right.append(padTop, padArea);
    bottom.append(left, right);
    body.append(bottom);
    const foot = el('div', 'mk3-foot');
    for (const name of ['master', 'headphone', 'mic']) { const label = el('label', '', name === 'headphone' ? 'Phones ' : name[0].toUpperCase() + name.slice(1) + ' '); const range = el('input'); range.type = 'range'; range.min = '0'; range.max = '1'; range.step = '0.01'; range.value = '0.7'; range.dataset.mk3Knob = name; label.append(range); foot.append(label); }
    const pedal = el('label', '', 'Pedal '); const box = el('input'); box.type = 'checkbox'; box.dataset.mk3Pedal = '1'; pedal.append(box); foot.append(pedal);
    foot.append(el('span', 'mk3-hint', 'Shift-click = SHIFT + button · Alt/right-click latches'));
    body.append(foot);
    root.append(body);
    return root;
  }

  // ---- Input wiring ---------------------------------------------------------------------------------
  function press(name, pressed) { send({ type: 'button', name, pressed }); }
  function bind(root) {
    root.querySelector('#mk3-sim-close').onclick = close;
    root.querySelector('#mk3-sim-zoom').onclick = () => { sim.scale = sim.scale === 1 ? .75 : sim.scale === .75 ? .6 : 1; root.style.setProperty('--mk3-scale', String(sim.scale)); };
    root.addEventListener('contextmenu', event => { if (event.target.closest('[data-mk3-button],[data-mk3-pad]')) event.preventDefault(); });
    for (const node of root.querySelectorAll('[data-mk3-button]')) {
      const name = node.dataset.mk3Button;
      let withShift = false, down = false;
      node.addEventListener('pointerdown', event => {
        event.preventDefault();
        if (name === 'shift') { sim.shiftSticky = !sim.shiftSticky; press('shift', sim.shiftSticky); node.classList.toggle('sticky', sim.shiftSticky); return; }
        if (event.altKey || event.button === 2) { const on = !sim.sticky.has(name); if (on) sim.sticky.add(name); else sim.sticky.delete(name); node.classList.toggle('sticky', on); press(name, on); return; }
        if (sim.sticky.has(name)) { sim.sticky.delete(name); node.classList.remove('sticky'); press(name, false); return; }
        withShift = event.shiftKey && !sim.shiftSticky; if (withShift) press('shift', true);
        down = true; press(name, true); node.setPointerCapture?.(event.pointerId);
      });
      const up = () => { if (!down) return; down = false; press(name, false); if (withShift) { press('shift', false); withShift = false; } };
      node.addEventListener('pointerup', up); node.addEventListener('pointercancel', up);
    }
    for (const knob of root.querySelectorAll('[data-mk3-encoder]')) {
      const index = Number(knob.dataset.mk3Encoder), target = 'encoder' + (index + 1);
      let drag = null;
      knob.addEventListener('pointerenter', () => send({ type: 'touch', target, touched: true }));
      knob.addEventListener('pointerleave', () => { if (!drag) send({ type: 'touch', target, touched: false }); });
      knob.addEventListener('wheel', event => { event.preventDefault(); send({ type: 'encoder', index, delta: -event.deltaY / 1500 }); }, { passive: false });
      knob.addEventListener('pointerdown', event => { event.preventDefault(); drag = { y: event.clientY }; knob.setPointerCapture?.(event.pointerId); send({ type: 'touch', target, touched: true }); });
      knob.addEventListener('pointermove', event => { if (!drag) return; const dy = drag.y - event.clientY; if (Math.abs(dy) < 1) return; drag.y = event.clientY; send({ type: 'encoder', index, delta: dy / (event.shiftKey ? 2000 : 250) }); });
      const end = () => { if (!drag) return; drag = null; send({ type: 'touch', target, touched: false }); };
      knob.addEventListener('pointerup', end); knob.addEventListener('pointercancel', end);
    }
    const main = root.querySelector('[data-mk3-main]');
    let wheel = 0, pushed = false;
    main.addEventListener('pointerenter', () => send({ type: 'touch', target: 'main', touched: true }));
    main.addEventListener('pointerleave', () => send({ type: 'touch', target: 'main', touched: false }));
    main.addEventListener('wheel', event => { event.preventDefault(); wheel += event.deltaY; while (Math.abs(wheel) >= 60) { const d = wheel > 0 ? 1 : -1; wheel -= d * 60; send({ type: 'main', delta: d }); } }, { passive: false });
    main.addEventListener('pointerdown', event => { event.preventDefault(); pushed = true; main.setPointerCapture?.(event.pointerId); press('encoderPush', true); });
    const release = () => { if (pushed) { pushed = false; press('encoderPush', false); } };
    main.addEventListener('pointerup', release); main.addEventListener('pointercancel', release);
    for (const pad of root.querySelectorAll('[data-mk3-pad]')) {
      const n = Number(pad.dataset.mk3Pad); let down = false;
      pad.addEventListener('pointerdown', event => {
        event.preventDefault(); const rect = pad.getBoundingClientRect(), y = rect.height ? (event.clientY - rect.top) / rect.height : .3;
        const velocity = Math.max(1, Math.min(127, Math.round(127 - y * 100)));
        let shift = false; if (event.shiftKey && !sim.shiftSticky) { shift = true; press('shift', true); }
        down = true; pad.dataset.shift = shift ? '1' : ''; pad.setPointerCapture?.(event.pointerId);
        send({ type: 'pad', pad: n, phase: 'down', velocity, pressure: velocity / 127 });
      });
      pad.addEventListener('pointermove', event => { if (down && event.pressure) send({ type: 'pad', pad: n, phase: 'pressure', pressure: event.pressure }); });
      const up = () => { if (!down) return; down = false; send({ type: 'pad', pad: n, phase: 'up', pressure: 0 }); if (pad.dataset.shift) { press('shift', false); pad.dataset.shift = ''; } };
      pad.addEventListener('pointerup', up); pad.addEventListener('pointercancel', up);
    }
    const strip = root.querySelector('[data-mk3-strip]');
    let stripDown = false;
    const stripValue = event => { const rect = strip.getBoundingClientRect(); return rect.width ? Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) : .5; };
    strip.addEventListener('pointerdown', event => { event.preventDefault(); stripDown = true; strip.setPointerCapture?.(event.pointerId); send({ type: 'touch', target: 'strip', touched: true }); send({ type: 'strip', value: stripValue(event) }); });
    strip.addEventListener('pointermove', event => { if (stripDown) send({ type: 'strip', value: stripValue(event) }); });
    const stripUp = () => { if (!stripDown) return; stripDown = false; send({ type: 'touch', target: 'strip', touched: false }); };
    strip.addEventListener('pointerup', stripUp); strip.addEventListener('pointercancel', stripUp);
    for (const range of root.querySelectorAll('[data-mk3-knob]')) range.addEventListener('input', () => send({ type: 'knob', name: range.dataset.mk3Knob, value: Number(range.value) }));
    root.querySelector('[data-mk3-pedal]').addEventListener('change', event => send({ type: 'pedal', down: event.target.checked }));
  }

  // ---- Output rendering ---------------------------------------------------------------------------
  function drawOps(canvas, ops) {
    const g = canvas.getContext?.('2d'); if (!g) return;
    g.save(); g.setTransform(1, 0, 0, 1, 0, 0); g.textBaseline = 'top';
    for (const op of ops || []) {
      switch (op.op) {
        case 'clear': g.fillStyle = op.color; g.fillRect(0, 0, canvas.width, canvas.height); break;
        case 'rect': g.fillStyle = op.color; if (op.radius && g.roundRect) { g.beginPath(); g.roundRect(op.x, op.y, op.w, op.h, op.radius); g.fill(); } else g.fillRect(op.x, op.y, op.w, op.h); break;
        case 'frame': g.strokeStyle = op.color; g.lineWidth = op.width || 1; g.strokeRect(op.x + .5, op.y + .5, op.w - 1, op.h - 1); break;
        case 'line': g.strokeStyle = op.color; g.lineWidth = op.width || 1; g.beginPath(); g.moveTo(op.x1 + .5, op.y1 + .5); g.lineTo(op.x2 + .5, op.y2 + .5); g.stroke(); break;
        case 'text': {
          g.font = `${op.weight === 'bold' ? '600 ' : ''}${op.size}px ${op.mono ? 'ui-monospace,Menlo,monospace' : '-apple-system,BlinkMacSystemFont,sans-serif'}`;
          g.fillStyle = op.color; g.textAlign = op.align || 'left';
          if (op.width) {
            // Contract: the text is fitted (ellipsis) and aligned inside [x, x + width], like the native renderer.
            let text = String(op.text);
            if (g.measureText(text).width > op.width) { while (text.length > 1 && g.measureText(text + '…').width > op.width) text = text.slice(0, -1); text += '…'; }
            const anchor = op.align === 'center' ? op.x + op.width / 2 : op.align === 'right' ? op.x + op.width : op.x;
            g.save(); g.beginPath(); g.rect(op.x - 1, op.y - 2, op.width + 2, op.size + 6); g.clip(); g.fillText(text, anchor, op.y); g.restore();
          } else g.fillText(op.text, op.x, op.y);
          break;
        }
        case 'bar': {
          g.fillStyle = op.background; g.fillRect(op.x, op.y, op.w, op.h); g.fillStyle = op.color;
          if (op.h > op.w) { const h = op.h * op.value; g.fillRect(op.x, op.y + op.h - h, op.w, h); } else g.fillRect(op.x, op.y, op.w * op.value, op.h);
          break;
        }
        case 'arc': {
          const start = Math.PI * .75, sweep = Math.PI * 1.5;
          g.lineWidth = op.width || 3; g.lineCap = 'round';
          g.strokeStyle = op.background; g.beginPath(); g.arc(op.x, op.y, op.r, start, start + sweep); g.stroke();
          if (op.value > 0) { g.strokeStyle = op.color; g.beginPath(); g.arc(op.x, op.y, op.r, start, start + sweep * op.value); g.stroke(); }
          break;
        }
        default: break;
      }
    }
    g.restore();
  }
  function lit(node, value, color = '#f2a65a') {
    const v = Math.max(0, Math.min(1, Number(value) || 0));
    node.style.setProperty('--mk3-led', v ? color : 'transparent');
    node.style.setProperty('--mk3-led-alpha', String(v));
    node.classList.toggle('lit', v >= .5);
  }
  function paint(frame) {
    const root = sim.root; if (!root) return;
    sim.lastFrame = frame;
    const { leds, screens } = frame;
    if (frame.screenChanged || !root.dataset.painted) { for (const canvas of root.querySelectorAll('[data-mk3-screen]')) drawOps(canvas, screens[canvas.dataset.mk3Screen]); root.dataset.painted = '1'; }
    for (const node of root.querySelectorAll('[data-mk3-button]')) { const name = node.dataset.mk3Button; if (name.startsWith('group')) continue; if (leds.buttons[name] !== undefined) lit(node, leds.buttons[name], name === 'rec' ? '#ff4a3a' : name === 'play' ? '#5ad16a' : name.startsWith('display') ? '#ffffff' : '#f2a65a'); }
    // Group buttons name the bank's entries (tracks and track groups); in PAD MODE a group's pads name its tracks.
    const controller = MK3(), page = controller?.page(), state = controller?.state, project = window.ST?.state?.project, cursor = window.ST?.state?.track;
    const list = controller?.entries?.() || [], bank = list.slice((state?.bank || 0) * 8, (state?.bank || 0) * 8 + 8);
    const group = state?.padMode === 'pad' && !state?.padAlt ? controller?.groupOf?.() : null;
    const members = group ? list.find(entry => entry.group === group)?.members.slice((state.padBank?.pad || 0) * 16, (state.padBank?.pad || 0) * 16 + 16) || [] : [];
    root.querySelectorAll('[data-mk3-button^="group"]').forEach((node, i) => {
      const color = leds.groups[i], entry = bank[i], name = entry ? entry.group ? entry.group.name : project?.tracks?.[entry.index]?.name || '' : '';
      const selected = !!entry && entry.members.includes(cursor);
      node.style.background = color || ''; node.classList.toggle('off', !color); node.classList.toggle('selected', selected); node.classList.toggle('named', !!name);
      node.querySelector('.mk3-group-name').textContent = name;
      node.title = name ? `Group ${LABELS[node.dataset.mk3Button]} · ${name}` : node.dataset.mk3Button;
      node.setAttribute('aria-label', node.title); node.setAttribute('aria-pressed', String(selected));
    });
    for (const pad of root.querySelectorAll('[data-mk3-pad]')) {
      const index = Number(pad.dataset.mk3Pad) - 1, color = leds.pads[index], member = members[index], name = member !== undefined ? project?.tracks?.[member]?.name || '' : '';
      pad.style.background = color || ''; pad.classList.toggle('off', !color); pad.classList.toggle('selected', member !== undefined && member === cursor);
      pad.querySelector('.mk3-pad-name').textContent = name;
      pad.title = name ? `Pad ${index + 1} · ${name}` : `Pad ${index + 1}`;
      pad.setAttribute('aria-label', pad.title);
    }
    root.querySelectorAll('[data-mk3-strip] i').forEach((node, i) => { node.style.opacity = String(.12 + .88 * (leds.strip[i] || 0)); });
    for (const [dir, color] of Object.entries(leds.nav || {})) { const node = root.querySelector(`[data-mk3-button="encoder${dir[0].toUpperCase()}${dir.slice(1)}"]`); if (node) node.style.color = color || ''; }
    const status = root.querySelector('#mk3-sim-status');
    if (status && state) status.textContent = `${state.screen}${page?.overlay ? ' · ' + page.id : ''}${group ? ' · ' + group.name : ''} · pads: ${state.padMode}${state.shift ? ' · SHIFT' : ''}${state.connected ? ' · hardware connected' : ''}`;
  }

  function open() {
    if (sim.root) return sim.root;
    if (!MK3()) return null;
    sim.root = build(); document.body.append(sim.root); bind(sim.root);
    sim.unsubscribe = MK3().subscribe(paint);
    MK3().publish();
    return sim.root;
  }
  function close() {
    if (!sim.root) return;
    sim.unsubscribe?.(); sim.unsubscribe = null;
    for (const pad of sim.heldPads) send({ type: 'pad', pad, phase: 'up', pressure: 0, cancelled: true, targetOnly: true });
    for (const target of sim.heldTouches) send({ type: 'touch', target, touched: false, cancelled: true, targetOnly: true });
    if (sim.pedalDown) send({ type: 'pedal', down: false });
    for (const name of [...sim.heldButtons].filter(name => name !== 'shift')) send({ type: 'button', name, pressed: false, cancelled: true, targetOnly: true });
    if (sim.heldButtons.has('shift')) send({ type: 'button', name: 'shift', pressed: false, cancelled: true, targetOnly: true });
    sim.sticky.clear(); sim.shiftSticky = false;
    sim.root.remove(); sim.root = null;
  }
  function toggle() { if (sim.root) close(); else open(); return !!sim.root; }
  window.STMK3Sim = { open, close, toggle, drawOps, get isOpen() { return !!sim.root; }, get lastFrame() { return sim.lastFrame; } };
  const auto = () => { try { if (/[?&]mk3sim\b/.test(window.location?.search || '')) setTimeout(open, 300); } catch { /* no location */ } };
  if (document.readyState === 'loading') document.addEventListener?.('DOMContentLoaded', auto); else auto();
})();
