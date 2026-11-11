/* Maschine MK3 screens: turns the controller page model (mk3.js) into two 480 × 272 display lists.
   Ops follow docs/MASCHINE_MK3.md (clear, rect, frame, line, text, bar, arc). Every coordinate is clipped to
   the screen, so a display list never draws outside 480 × 272.

   Layout of each screen (both share it, so the 8 display buttons and 8 encoders line up across the pair):
     y   0– 24  display-button labels, 4 columns of 120 px, right under the physical buttons
     y  24– 46  title bar (left: mode, track, pattern · right: position, tempo, transport)
     y  48–202  page content
     y 204–272  encoder labels, values and value bars, 4 columns of 120 px, right above the encoders
   Text with a width is laid out inside [x, x + width] (the renderer truncates it with an ellipsis). */
(() => {
  'use strict';
  const W = 480, H = 272, COL = 120;
  // Rows a list shows on one screen; the controller (mk3-lists.js) scrolls its lists by the same count.
  const LIST_ROWS = 7;
  const BTN_H = 24, TITLE_Y = BTN_H, TITLE_H = 22, CONTENT_Y = TITLE_Y + TITLE_H + 2, KNOBS_Y = 204, CONTENT_H = KNOBS_Y - CONTENT_Y - 2;
  // Dark theme of the app (styles.css), deepened for the LCD; accent #ed7144.
  const C = { bg: '#0f0f0d', panel: '#1d1d1a', panel2: '#2a2a25', raised: '#34342e', line: '#3b3a32', dim: '#77736a', hint: '#8e897d', muted: '#b0aa9d', text: '#ece7dc', bright: '#ffffff',
    accent: '#ed7144', accentDim: '#5a3222', amber: '#cfad67', red: '#e0604e', green: '#8cc06a', blue: '#a1b0b8', play: '#1f3a2c', select: '#2c3442', black: '#000000', white: '#ffffff' };
  const FONT = { button: 13, label: 12, value: 15, body: 13, small: 12, title: 13, mono: 13 };
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const num = v => Number.isFinite(Number(v)) ? Number(v) : 0;
  const pad2 = n => String(n).padStart(2, '0'), pad3 = n => String(n).padStart(3, '0');

  // Approximate advance widths (em) of the system font, for laying out chips; the renderer truncates exactly.
  const NARROW = /[iljI.,:;'|!·()\[\] ]/, WIDE = /[mwMW@%—]/;
  function textWidth(text, size, bold, mono) {
    const s = String(text ?? '');
    if (mono) return s.length * size * .6;
    let em = 0; for (const ch of s) em += NARROW.test(ch) ? .3 : WIDE.test(ch) ? .85 : /[A-Z0-9]/.test(ch) ? .64 : .54;
    return em * size * (bold ? 1.06 : 1);
  }

  // Greedy word wrap on the estimated widths (slightly conservative so the renderer never truncates).
  function wrap(text, size, width, bold) {
    const words = String(text ?? '').split(/\s+/).filter(Boolean), lines = [];
    let line = '';
    for (const word of words) {
      const next = line ? line + ' ' + word : word;
      if (line && textWidth(next, size, bold) * 1.04 > width) {
        lines.push(line);
        line = word;
      } else {
        line = next;
      }
    }
    if (line) lines.push(line);
    return lines;
  }
  /** Draws wrapped paragraphs from y; returns the y after the last line. First paragraph in `first` colour. */
  function paragraphs(g, x, y, list, size, width, colors, lineH = size + 7, maxY = KNOBS_Y - 4) {
    list.forEach((text, i) => {
      for (const line of wrap(text, size, width)) {
        if (y + size > maxY) return;
        g.text(x, y, line, size, i ? colors[1] : colors[0], { width });
        y += lineH;
      }
      y += 3;
    });
    return y;
  }
  function canvas() {
    const ops = [];
    const cx = x => Math.round(clamp(num(x), 0, W)), cy = y => Math.round(clamp(num(y), 0, H));
    const box = (x, y, w, h) => {
      const x0 = cx(x),
        y0 = cy(y),
        x1 = cx(num(x) + num(w)),
        y1 = cy(num(y) + num(h));
      return { x: x0, y: y0, w: Math.max(0, x1 - x0), h: Math.max(0, y1 - y0) };
    };
    return {
      ops,
      clear(color = C.bg) { ops.push({ op: 'clear', color }); },
      rect(x, y, w, h, color, radius) {
        const b = box(x, y, w, h);
        if (!b.w || !b.h || !color) return;
        const op = { op: 'rect', ...b, color };
        if (radius) op.radius = radius;
        ops.push(op);
      },
      frame(x, y, w, h, color, width = 1) { const b = box(x, y, w, h); if (!b.w || !b.h) return; ops.push({ op: 'frame', ...b, color, width }); },
      line(x1, y1, x2, y2, color, width = 1) { ops.push({ op: 'line', x1: cx(x1), y1: cy(y1), x2: cx(x2), y2: cy(y2), color, width }); },
      /* x is the left edge (align left), the centre (center) or the right edge (right); `width` is the box the
         text is fitted into, centred on / ending at x for center / right. */
      text(x, y, text, size = FONT.body, color = C.text, options = {}) {
        const s = String(text ?? '').slice(0, 160); if (!s) return;
        const align = options.align || 'left', width = options.width ? Math.max(1, Math.round(options.width)) : 0;
        const op = { op: 'text', x: 0, y: clamp(Math.round(num(y)), 0, H - size), text: s, size, color };
        if (width) {
          let left = align === 'center' ? num(x) - width / 2 : align === 'right' ? num(x) - width : num(x);
          let right = left + width; left = clamp(left, 0, W); right = clamp(right, 0, W);
          if (right - left < 4) return;
          op.x = Math.round(left); op.width = Math.round(right - left);
        } else op.x = cx(x);
        if (options.weight === 'bold') op.weight = 'bold';
        if (align !== 'left') op.align = align;
        if (options.mono) op.mono = true;
        ops.push(op);
      },
      bar(x, y, w, h, value, color, background = C.panel2) {
        const b = box(x, y, w, h);
        if (!b.w || !b.h) return;
        ops.push({ op: 'bar', ...b, value: clamp(num(value), 0, 1), color, background });
      },
      arc(x, y, r, value, color, background = C.line, width = 3) {
        const rr = Math.min(r, num(x), W - num(x), num(y), H - num(y));
        if (rr < 2) return;
        ops.push({ op: 'arc', x: cx(x), y: cy(y), r: Math.round(rr), value: clamp(num(value), 0, 1), color, background, width });
      }
    };
  }
  const isHex = c => /^#[0-9a-f]{6}$/i.test(c || '');
  function scaleColor(color, k) {
    if (!isHex(color) || k >= .999) return color;
    return '#' + [1, 3, 5].map(i => Math.round(parseInt(color.slice(i, i + 2), 16) * k).toString(16).padStart(2, '0')).join('');
  }
  function mix(color, other, t) {
    if (!isHex(color) || !isHex(other)) return isHex(color) ? color : isHex(other) ? other : C.panel2;
    return '#'
        + [1, 3, 5].map(i => Math.round(parseInt(color.slice(i, i + 2), 16) * (1 - t)
            + parseInt(other.slice(i, i + 2), 16) * t).toString(16).padStart(2,
            '0')).join('');
  }
  const tint = (color, t = .3) => mix(C.bg, isHex(color) ? color : C.accent, t);
  const ink = color => {
    if (!isHex(color)) return C.black;
    const [r, g, b] = [1, 3, 5].map(i => parseInt(color.slice(i, i + 2), 16));
    return r * .299 + g * .587 + b * .114 > 140 ? C.black : C.white;
  };

  // Small shared pieces.
  function chip(g, x, y, label, color, size = FONT.small, options = {}) {
    const w = Math.ceil(textWidth(label, size, true) + 12), h = size + 6;
    const left = options.align === 'right' ? x - w : x;
    g.rect(left, y, w, h, color, 3);
    g.text(left + w / 2, y + 3, label, size, options.ink || ink(color), { align: 'center', width: w - 4, weight: 'bold' });
    return w;
  }
  function swatch(g, x, y, color, size = 10) { g.rect(x, y, size, size, isHex(color) ? color : C.accent, 2); }
  function heading(g, x, y, text, width = W - 16, color = C.muted, align = 'left') {
    g.text(x, y, String(text || '').toUpperCase(), 11, color, { width, weight: 'bold', align });
  }
  function hints(g, lines, y, color = C.hint) {
    lines.filter(Boolean).forEach((line, i) => g.text(10, y + i * 18, line, FONT.small, color, { width: W - 20 }));
  }
  function valueBar(g, x, y, w, h, value, color, bipolar) {
    g.rect(x, y, w, h, C.panel2, h / 2);
    const v = clamp(num(value), 0, 1);
    if (bipolar) { const mid = x + w / 2, end = x + v * w; g.rect(Math.min(mid, end), y, Math.max(2, Math.abs(end - mid)), h, color,
        h / 2); g.rect(mid - 1, y - 2, 2, h + 4, C.muted); }
    else if (v > 0) g.rect(x, y, Math.max(h, v * w), h, color, h / 2);
  }

  // ---- Chrome: display-button labels, title bar, encoder labels ----------------------------------
  function drawButtons(g, buttons) {
    buttons.forEach((b, i) => {
      const x = i * COL;
      if (i) g.line(x, 5, x, BTN_H - 5, C.line);
      if (!b || !b.label) return;
      const color = isHex(b.color) ? b.color : null;
      let fg = b.enabled ? C.text : C.dim, weight = 'regular';
      if (color && b.select) {
        // Selectable columns / strips (Home, MIXER): bright = selected; a colour strip under every label; M / S flags.
        if (b.on) g.rect(x + 4, 2, COL - 8, BTN_H - 4, mix(C.bg, color, .42), 4);
        g.rect(x + 8, BTN_H - 4, COL - 16, 2, b.mute ? C.dim : color, 1);
        const flag = b.mute ? ['M', C.red] : b.solo ? ['S', C.amber] : null;
        if (flag) g.text(x + 10, 5, flag[0], FONT.button, flag[1], { weight: 'bold', width: 14 });
        g.text(x + COL / 2 + (flag ? 7 : 0), 5, b.label, FONT.button, b.on ? C.text : b.mute ? C.dim : C.muted, {
          align: 'center',
          width: COL - (flag ? 30 : 16),
          weight: b.on ? 'bold' : 'regular'
        });
        return;
      }
      if (color) {
        // Track-coloured toggles (mixer mutes): coloured when on, dark with a red M when off/muted.
        if (b.on) { g.rect(x + 4, 2, COL - 8, BTN_H - 4, mix(C.bg, color, .42), 4); g.rect(x + 4, BTN_H - 5, COL - 8, 3, color, 1); fg = C.text; }
        else { g.frame(x + 4, 2, COL - 8, BTN_H - 4, C.line, 1); fg = C.dim; if (b.mute) g.text(x + 10, 5, 'M', FONT.button, C.red,
            { weight: 'bold', width: 14 }); }
      } else if (b.on) { g.rect(x + 4, 2, COL - 8, BTN_H - 4, b.mute ? C.panel2 : C.accent, 4); fg = b.mute ? C.muted : C.black; weight = 'bold'; }
      const inset = color && !b.on && b.mute ? 22 : 8;
      g.text(x + COL / 2 + (inset - 8) / 2, 5, b.label, FONT.button, fg, { align: 'center', width: COL - inset * 2 + (inset - 8), weight });
    });
  }
  function drawTitle(g, side, t) {
    g.rect(0, TITLE_Y, W, TITLE_H, C.panel);
    const y = TITLE_Y + 4;
    if (side === 'left') {
      const w = chip(g, 3, TITLE_Y + 2, t.mode || '', C.accent, FONT.title, { ink: C.black });
      let x = 3 + w + 8;
      swatch(g, x, TITLE_Y + 6, t.track.color, 10); x += 15;
      const trackText = `${pad2(t.track.index + 1)} ${t.track.name}`;
      const pat = `P${pad2(t.pattern.number)} ${t.pattern.name}`;
      const trackW = Math.min(textWidth(trackText, FONT.title) + 6, 190);
      g.text(x, y, trackText, FONT.title, C.text, { width: trackW });
      const patX = x + trackW + 10;
      if (W - 6 - patX > 40) g.text(W - 6, y, pat, FONT.title, C.muted, { align: 'right', width: W - 6 - patX });
      return;
    }
    // Right: position, tempo, page indicator, playback mode, then status flags packed from the right edge.
    g.text(8, y, t.position, FONT.title, C.text, { mono: true, width: 112 });
    g.text(126, y, `${t.bpm.toFixed(1)} BPM`, FONT.title, C.text, { width: 86 });
    let left = 218;
    if (t.pages?.count > 1) left = pageDots(g, left, TITLE_Y + 8, t.pages.count, t.pages.index) + 8;
    let x = W - 6;
    // Transport icon.
    if (t.playing) { g.rect(x - 11, TITLE_Y + 5, 4, 12, C.green, 1); g.rect(x - 4, TITLE_Y + 5, 4, 12, C.green, 1); }
    else g.rect(x - 11, TITLE_Y + 6, 10, 10, C.muted, 1);
    x -= 18;
    if (t.recording) { g.rect(x - 11, TITLE_Y + 5, 11, 11, C.red, 6); x -= 18; }
    if (t.dirty) { g.rect(x - 7, TITLE_Y + 8, 6, 6, C.amber, 3); x -= 14; }
    if (t.shift) x -= chip(g, x, TITLE_Y + 3, 'SHIFT', C.amber, 11, { align: 'right', ink: C.black }) + 6;
    if (t.auto && t.auto !== 'read') x -= chip(g, x, TITLE_Y + 3, t.auto.toUpperCase(), C.red, 11, { align: 'right', ink: C.white }) + 6;
    const mode = t.songMode ? 'SONG' : 'PATTERN';
    if (x - left > textWidth(mode, 11, true) + 12) chip(g, x, TITLE_Y + 3, mode, C.raised, 11, { align: 'right', ink: C.muted });
  }
  /** Page indicator of the ◄ ► grammar: one dot per page (current filled), or "3 / 20" for long page lists. */
  function pageDots(g, x, y, count, index) {
    if (count > 8) { const label = `${index + 1} / ${count}`, w = textWidth(label, FONT.small) + 18; g.rect(x, y + 1, 6, 6, C.accent, 3); g.text(x + 10,
        y - 3, label, FONT.small, C.muted, { width: w }); return x + 10 + w; }
    for (let i = 0; i < count; i++) { if (i === index) g.rect(x + i * 11, y, 7, 7, C.accent, 3); else g.frame(x + i * 11, y, 7, 7, C.dim, 1); }
    return x + count * 11;
  }
  /** Pattern-position bar under the title on tracker pages: one 960 px bar across both screens. */
  function drawProgress(g, side, pr) {
    if (!pr || !(pr.length > 0)) return;
    const y = TITLE_Y + TITLE_H - 3, offset = side === 'left' ? 0 : W, at = row => (row + .5) / pr.length * W * 2 - offset;
    g.rect(0, y, W, 3, C.panel2);
    const cursor = at(pr.row);
    if (cursor > 0) g.rect(0, y, Math.min(W, cursor), 3, C.accentDim);
    if (cursor >= -2 && cursor <= W + 2) g.rect(cursor - 2, y, 4, 3, C.accent);
    if (pr.playhead >= 0) { const play = at(pr.playhead); if (play >= -2 && play <= W + 2) g.rect(play - 1, y - 1, 3, 4, C.green); }
  }
  /* Encoder modes (VOLUME, SWING, TEMPO, NOTE REPEAT, EVENTS, LOCK): a header strip right above the encoder labels;
     the page above stays. Left: mode chip, target and value; right: what the 4-D encoder and encoders do. */
  const HEADER_H = 22, HEADER_Y = KNOBS_Y - HEADER_H - 1;
  function drawHeader(g, side, h) {
    g.rect(0, HEADER_Y, W, HEADER_H, C.panel);
    g.line(0, HEADER_Y, W, HEADER_Y, h.lock ? C.amber : C.accent);
    if (side === 'left') {
      const w = chip(g, 4, HEADER_Y + 3, h.title || '', h.lock ? C.amber : C.accent, 11, { ink: C.black });
      const valueW = h.value ? Math.min(170, textWidth(h.value, FONT.body, true) + 8) : 0;
      g.text(4 + w + 8, HEADER_Y + 4, h.target || '', FONT.body, C.text, { width: W - w - 24 - valueW });
      if (h.value) g.text(W - 6, HEADER_Y + 4, h.value, FONT.body, C.text, { align: 'right', width: valueW, weight: 'bold' });
      return;
    }
    g.text(8, HEADER_Y + 5, h.hint || '', FONT.small, C.hint, { width: W - 16 });
  }
  function drawKnobs(g, knobs, offset, model) {
    g.line(0, KNOBS_Y, W, KNOBS_Y, C.line);
    for (let i = 0; i < 4; i++) {
      const k = knobs[offset + i], x = i * COL;
      if (i) g.line(x, KNOBS_Y + 8, x, H - 8, C.line);
      if (!k) continue;
      const touched = model.touch?.index === offset + i;
      if (touched) g.rect(x + 3, KNOBS_Y + 3, COL - 6, H - KNOBS_Y - 6, C.panel2, 5);
      // A greyed-out knob shows the default a field gets (Home on an empty row); it is never blank.
      g.text(x + COL / 2, KNOBS_Y + 7, k.label, FONT.label, touched ? C.accent : k.dim ? C.dim : C.muted, { align: 'center', width: COL - 12 });
      g.text(x + COL / 2, KNOBS_Y + 26, k.value, FONT.value, k.dim ? C.dim : C.text, {
        align: 'center',
        width: COL - 10,
        weight: k.dim ? 'regular' : 'bold'
      });
      const color = k.lockable ? C.amber : C.accent;
      if (k.norm != null) valueBar(g, x + 14, KNOBS_Y + 52, COL - 28, 5, k.norm, color, k.bipolar);
      else { g.line(x + 30, KNOBS_Y + 54, x + COL - 30, KNOBS_Y + 54, C.panel2, 2); }
      if (k.lockable) g.text(x + COL - 8, KNOBS_Y + 7, 'L', 11, C.amber, { align: 'right', width: 12, weight: 'bold' });
    }
  }
  function drawTouch(g, model) {
    const t = model.touch; if (!t || t.list) return;
    if (t.options?.length > 2) return drawChoices(g, t, model.page?.data?.header ? HEADER_H + 2 : 0);
    // A compact strip just above the encoders: the content above stays visible while turning.
    const h = 40, x = 8, y = KNOBS_Y - h - 4 - (model.page?.data?.header ? HEADER_H + 2 : 0), w = W - 16;
    g.rect(x, y, w, h, C.panel, 6);
    g.frame(x, y, w, h, C.accent, 1);
    g.text(x + 12, y + 11, t.name, 15, C.muted, { width: w * .5 - 16 });
    g.text(x + w * .5, y + 7, t.value, 22, C.text, { align: 'right', width: w * .5 - 12, weight: 'bold' });
    if (t.norm != null) valueBar(g, x + 12, y + h - 7, w - 24, 3, t.norm, C.accent, false);
  }
  /** A touched list parameter (enum knob) shows its choices around the current one, above its encoder. */
  function drawChoices(g, t, lift = 0) {
    const rows = Math.min(5, t.options.length), rowH = 18, w = 210, h = 22 + rows * rowH + 6, col = t.index % 4;
    const x = clamp(col * COL + COL / 2 - w / 2, 6, W - w - 6), y = KNOBS_Y - h - 4 - lift;
    g.rect(x, y, w, h, C.panel, 6); g.frame(x, y, w, h, C.accent, 1);
    drawList(g, { title: t.name, items: t.options.map(name => ({ name })), selected: t.selected, rows, active: true },
        { x: x + 2, w: w - 4, y: y + 3, rowH });
  }
  function drawFlash(g, text, lift = 0) {
    const w = Math.min(W - 40, textWidth(text, 18, true) + 48), x = (W - w) / 2, y = CONTENT_Y + CONTENT_H - 52 - lift;
    g.rect(x, y, w, 40, C.accent, 8);
    g.text(W / 2, y + 10, text, 18, C.black, { align: 'center', width: w - 20, weight: 'bold' });
  }

  // ---- Content drawers (by page data kind) ----------------------------------------------------------
  const NOTE_NAMES = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
  // A note's gate in rows, or "—" for none (it plays until the next note, OFF or CUT). A step saved
  // without a gate key is the default 0.8 rows long.
  function gateLabel(s, unit = '') {
    const gate = s.gate === undefined || s.gate === null ? .8 : Number(s.gate);
    return gate > 0 ? gate.toFixed(2) + unit : '—';
  }
  function noteLabel(n) {
    const app = window.__supertracker?.noteText;
    if (app && n != null && n >= -2) return app(n);
    if (n === -1) return 'OFF';
    if (n === -2) return 'CUT';
    if (n == null || n < 0) return '···';
    return NOTE_NAMES[n % 12] + (Math.floor(n / 12) - 1);
  }
  const FX_NAMES = {
    probability: 'Probability',
    ratchet: 'Ratchet',
    microtiming: 'Microtiming',
    glide: 'Glide',
    reverse: 'Reverse',
    offset: 'Sample offset',
    arpeggio: 'Arpeggio',
    slice: 'Slice',
    position: 'Position',
    volume: 'Volume',
    pan: 'Pan',
    pitch: 'Pitch',
    cutoff: 'Cutoff',
    resonance: 'Resonance',
    delay: 'Delay send',
    reverb: 'Reverb send'
  };
  function fxValue(fx) {
    if (!fx) return '';
    if (fx.type === 'arpeggio') return Math.round(fx.value).toString(16).toUpperCase().padStart(2, '0');
    if (fx.type === 'position') return `${Math.round(Math.max(0, Math.min(1, Number(fx.value) || 0)) * 100)}%`;
    if (fx.type === 'plugin' || Math.abs(fx.value) < 1) return `${Math.round(fx.value * 100)}%`;
    return String(fx.value);
  }
  function fxLabel(fx) { if (!fx) return '—'; return `${fx.type === 'plugin' ? fx.name || 'Plug-in' : FX_NAMES[fx.type] || fx.type} ${fxValue(fx)}`; }

  /* Tracker grid (Home, MACRO, LOCK): 120 px columns exactly under the display buttons and above the encoders,
     four per screen (Home: columns 1–4 left, 5–8 right; MACRO: four columns on the left, step FX on the right).
     Eleven 14 px rows; each cell shows note · velocity · FX, and the cursor cell shows the field under the cursor
     (instrument / velocity, FX 1 / FX 2) highlighted. */
  const GRID_ROW_H = 14, GRID_FONT = 12;
  const compactFx = text => { const t = String(text ?? '').replace(/\s+/g, ''); return /^[·]*$/.test(t) ? '' : t; };
  function grid(g, side, data, model) {
    const gd = data.grid; if (!gd) return;
    if (side === 'right' && data.overview) return overview(g, data.overview);
    if (side === 'right' && data.fx) return stepFx(g, data, data.step, CONTENT_Y + 6);
    const offset = side === 'left' ? 0 : 4, cols = (gd.tracks || []).slice(offset, offset + 4), rows = gd.rows || [];
    const top = CONTENT_Y, fit = Math.floor((KNOBS_Y - 2 - top) / GRID_ROW_H), cursor = gd.cursor || {};
    const playIndex = rows.findIndex(r => r.playhead), focus = Math.max(0, playIndex >= 0 ? playIndex : rows.findIndex(r => r.row === cursor.row));
    const start = clamp(focus - Math.floor(fit / 2), 0, Math.max(0, rows.length - fit)), shown = rows.slice(start, start + fit);
    const charW = GRID_FONT * .6,
      width = n => Math.ceil(n * charW) + 2,
      current = t => t.current ?? (t.index === cursor.track || !!t.members?.includes(cursor.track));
    // The selected column gets a faint tint of its colour over the whole height.
    cols.forEach((t, c) => { if (current(t)) g.rect(c * COL + 1, top, COL - 2, shown.length * GRID_ROW_H, tint(t.color, .14)); });
    shown.forEach((r, i) => {
      const y = top + i * GRID_ROW_H, cursorRow = r.row === cursor.row;
      if (r.playhead) g.rect(0, y, W, GRID_ROW_H, C.play);
      else if (cursorRow) g.rect(0, y, W, GRID_ROW_H, C.panel);
      if (r.bar) g.line(0, y, W, y, C.line);
      if (side === 'left') {
        if (r.loop) g.rect(0, y, 3, GRID_ROW_H, C.amber);
        g.text(25, y + 1, pad3(r.row), GRID_FONT, r.playhead ? C.green : cursorRow ? C.accent : r.beat ? C.muted : C.dim, {
          align: 'right',
          mono: true,
          width: 22
        });
      }
      r.cells.slice(offset, offset + 4).forEach((cell, c) => {
        const col = cols[c] || {}, x = c * COL + (side === 'left' && c === 0 ? 29 : 6), right = (c + 1) * COL - 3;
        if (cell.event) g.rect(x - 3, y, right - x + 3, GRID_ROW_H, '#5a4a22');
        else if (cell.selected) g.rect(x - 3, y, right - x + 3, GRID_ROW_H, C.select);
        if (cell.summary != null) { if (cell.summary) g.text(x, y + 1, cell.summary, FONT.small, col.mute ? C.dim : C.text,
            { width: right - x }); return; }
        const active = cursorRow && cell.track === cursor.track, field = active ? cursor.column : -1;
        const parts = [cell.text[0], field === 1 ? cell.text[1] : cell.text[2], compactFx(field === 4 ? cell.text[4] : cell.text[3])];
        const hi = field < 0 ? -1 : field === 0 ? 0 : field <= 2 ? 1 : 2, widths = [width(3), width(3), width(6)];
        let px = x;
        parts.forEach((part, k) => {
          const text = k === 2 && !part && hi === 2 ? '·····' : part;
          if (hi === k) g.rect(px - 2, y + 1, widths[k] + 2, GRID_ROW_H - 2, C.accent, 2);
          if (text) {
            const empty = /^[·\s]*$/.test(String(text));
            const color = hi === k
                ? C.black
                : empty
                ? '#4b4941'
                : col.mute
                ? C.dim
                : k === 0
                ? cell.chord ? C.amber : /OFF|CUT|^=/.test(text) ? C.red : C.text
                : k === 2
                ? C.accent
                : C.muted;
            g.text(px, y + 1, text, GRID_FONT, color, { mono: true, width: Math.min(widths[k] + 2, right - px) });
          }
          px += widths[k] + 5;
        });
      });
    });
    for (let c = 1; c < 4; c++) g.line(c * COL, top, c * COL, top + shown.length * GRID_ROW_H, C.line);
  }
  function stepFx(g, data, s, y) {
    ['fx1', 'fx2'].forEach((key, i) => {
      const fx = s?.[key], bx = 8 + i * 236;
      g.rect(bx, y, 228, 78, fx ? C.panel2 : C.panel, 6);
      heading(g, bx + 10, y + 7, `FX ${i + 1}`, 100);
      g.text(bx + 10, y + 24, fx ? fx.type === 'plugin' ? fx.name || 'Plug-in' : FX_NAMES[fx.type] || fx.type : 'None', 17, fx ? C.text : C.dim, {
        width: 208,
        weight: 'bold'
      });
      if (fx) g.text(bx + 10, y + 48, fxValue(fx), 20, C.accent, { width: 208, weight: 'bold' });
    });
    g.text(10, y + 90, s
        ? `Note ${noteLabel(s.note)} · vel ${s.velocity ?? 100} · gate ${gateLabel(s)}`
        : 'Empty row: FX here lock a parameter without a note', FONT.body, s ? C.text : C.muted, {
      width: W - 20
    });
    hints(g, [data.lockHint ? null : 'Display buttons: quick effects', data.lockHint ? null : 'Encoders 5–7: probability, ratchet, microtiming'],
        y + 112);
    if (data.lockHint) g.text(10, y + 112, data.lockHint, FONT.small, C.amber, { width: W - 20, weight: 'bold' });
  }
  function overview(g, o) {
    const x0 = 10, w = W - 20, y0 = CONTENT_Y + 22, h = CONTENT_H - 30;
    heading(g, x0, CONTENT_Y + 4, `Pattern overview · ${o.length} rows`, 300);
    g.rect(x0, y0, w, h, C.panel, 4);
    if (o.loop) g.rect(x0 + o.loop.start / o.length * w, y0, (o.loop.end - o.loop.start + 1) / o.length * w, h, '#3a3220');
    const bw = w / Math.max(1, o.density.length);
    o.density.forEach((d, i) => {
      if (!d) return;
      const bh = Math.max(2, d / o.max * (h - 10));
      g.rect(x0 + i * bw + 1, y0 + h - 5 - bh, Math.max(1, bw - 2), bh, C.muted);
    });
    const at = row => x0 + (row + .5) / o.length * w;
    for (let bar = 16; bar < o.length; bar += 16) g.line(at(bar) - bw / 2, y0 + 2, at(bar) - bw / 2, y0 + h - 2, C.line);
    g.line(at(o.cursor), y0, at(o.cursor), y0 + h, C.accent, 2);
    if (o.playhead >= 0) g.line(at(o.playhead), y0, at(o.playhead), y0 + h, C.green, 2);
  }
  /* ---- The list component (every list on the screens; docs/MASCHINE_MK3.md "Lists") --------------------
     A list is { title, items:[{ name, right?, color?, on?, dim? }], selected, top, rows, total?, offset?, active?,
     busy?, empty? }. mk3.js keeps `top` (scrolloff), so the cursor travels before the list scrolls. The header
     shows the title and "12 / 340"; a scroll bar appears when the list is longer than its window. The active
     list's cursor is filled with the accent colour, an inactive list keeps an outlined cursor; `on` marks the
     applied value (current filter, active pattern). */
  const ROW_H = 19, LIST_HEAD = 18, PAGE_ROWS = 4;
  /** Centred fallback window when a list arrives without `top`. */
  function windowTop(count, selected, rows) { return count <= rows ? 0 : clamp(selected - Math.floor(rows / 2), 0, count - rows); }
  function drawList(g, list, options = {}) {
    const x = options.x ?? 0, w = options.w ?? W, y0 = options.y ?? CONTENT_Y, rowH = options.rowH || ROW_H, items = list?.items || [];
    const rows = Math.max(1, list?.rows || 7), count = items.length, selected = clamp(num(list?.selected), 0, Math.max(0, count - 1));
    const top = clamp(list?.top != null ? num(list.top) : windowTop(count, selected, rows), 0, Math.max(0, count - rows));
    const active = list?.active !== false, total = Math.max(count, num(list?.total)), offset = num(list?.offset);
    let y = y0;
    if (options.header !== false) {
      const label = list?.busy && !count
          ? 'searching…'
          : count
          ? `${(offset + selected + 1).toLocaleString('en-US')} / ${total.toLocaleString('en-US')}`
          : '';
      const labelW = Math.min(150, textWidth(label, FONT.small) + 12);
      heading(g, x + 8, y + 3, list?.title || '', w - labelW - 18, active ? C.muted : C.dim);
      if (label) g.text(x + w - 8, y + 2, `${list?.busy && count ? '… ' : ''}${label}`, FONT.small, list?.busy ? C.amber : C.dim, {
        align: 'right',
        width: labelW + 14
      });
      y += LIST_HEAD;
    }
    if (!count) { g.text(x + w / 2, y + Math.min(rows, 4) * rowH / 2 - 8, list?.busy ? 'Searching…' : list?.empty || 'Nothing here', 15, C.dim,
        { align: 'center', width: w - 24 }); return y + rows * rowH; }
    const scroll = total > rows, rightW = items.some(item => item.right) ? Math.round(Math.min(150, w * .32)) : 0, inner = w - (scroll ? 10 : 6);
    for (let i = 0; i < Math.min(rows, count - top); i++) {
      const item = items[top + i], ry = y + i * rowH, on = top + i === selected, ty = ry + (rowH - 2 - 14) / 2;
      if (on && active) g.rect(x + 3, ry, inner - 1, rowH - 2, C.accent, 4);
      else if (on) { g.rect(x + 3, ry, inner - 1, rowH - 2, C.panel2, 4); g.frame(x + 3, ry, inner - 1, rowH - 2, C.accentDim, 1); }
      if (item.on) g.rect(x + 4, ry + 3, 3, rowH - 8, on && active ? C.black : C.accent, 1);
      let tx = x + 12;
      if (item.color) { swatch(g, tx, ry + (rowH - 2) / 2 - 5, item.color, 10); tx += 16; }
      const fg = on && active ? C.black : item.dim ? C.dim : on || item.on ? C.text : C.muted;
      g.text(tx, ty, item.name ?? item.text ?? '', 14, fg, {
        width: x + inner - tx - (rightW ? rightW + 6 : 6),
        weight: on || item.on ? 'bold' : 'regular'
      });
      if (item.right) g.text(x + inner - 6, ry + (rowH - 2 - 12) / 2, String(item.right), FONT.small, on && active ? C.black : C.dim, {
        align: 'right',
        width: rightW
      });
    }
    if (scroll) {
      const track = rows * rowH - 2,
        thumb = Math.max(8, track * rows / total),
        pos = (track - thumb) * clamp((offset + top) / Math.max(1, total - rows), 0, 1);
      g.rect(x + w - 5, y, 3, track, C.panel2, 1); g.rect(x + w - 5, y + pos, 3, thumb, active ? C.muted : C.dim, 1);
    }
    return y + rows * rowH;
  }
  function chordPads(g, chords, y0 = CONTENT_Y + 116) {
    // Laid out like the pads: chord 1 bottom-left, 13 top-left.
    chords.slice(0, 16).forEach((name, i) => {
      const x = 6 + i % 4 * 117,
        y = y0 - Math.floor(i / 4) * 32;
      g.rect(x, y, 111, 28, C.panel2, 5);
      g.text(x + 8, y + 7, String(i + 1), 11, C.dim, { width: 18 });
      g.text(x + 60, y + 6, name, 15, C.text, { align: 'center', width: 76, weight: 'bold' });
    });
  }
  function list(g, side, data, model) {
    const items = data.items || [], selected = clamp(num(data.selected), 0, Math.max(0, items.length - 1));
    if (side === 'left') { drawList(g, data); return; }
    if (data.chords?.length) { heading(g, 10, CONTENT_Y + 3, `${data.big || 'Chord pads'} · pads`, W - 20,
        C.accent); chordPads(g, data.chords); return; }
    if (data.big) {
      heading(g, W / 2, CONTENT_Y + 10, model?.page?.title || data.title || '', W - 40, C.accent, 'center');
      const big = String(data.big), size = big.length > 18 ? 24 : big.length > 12 ? 30 : 36;
      g.text(W / 2, CONTENT_Y + 30 + (36 - size) / 2, big, size, C.text, { align: 'center', width: W - 24, weight: 'bold' });
      paragraphs(g, 12, CONTENT_Y + 84, data.lines || [], FONT.body, W - 24, [C.muted, C.hint], 19);
      return;
    }
    const item = items[selected];
    let y = CONTENT_Y + 6;
    if (item) {
      if (item.color) g.rect(10, CONTENT_Y + 8, 6, 46, item.color, 2);
      const x = item.color ? 24 : 10;
      heading(g, x, y, data.title || 'Selected', W - x - 10);
      g.text(x, CONTENT_Y + 22, item.name, 20, C.text, { weight: 'bold', width: W - x - 10 });
      g.text(x, CONTENT_Y + 50, item.detail || item.right || '', FONT.body, C.muted, { width: W - x - 10 });
      y = CONTENT_Y + 74;
    } else g.text(W / 2, CONTENT_Y + 40, data.empty || '', 14, C.dim, { align: 'center', width: W - 40 });
    if (data.info) paragraphs(g, 10, y, data.lines || [], FONT.small, W - 20, [C.muted, C.hint], 17, CONTENT_Y + CONTENT_H - 22);
    hints(g, [data.hint || 'Turn: move · push: choose'], CONTENT_Y + CONTENT_H - 20);
  }
  function statusLine(g, y, status, fallback) {
    const text = status?.text || fallback || '';
    g.text(10, y, text, FONT.small, status?.error ? C.red : status?.loading ? C.amber : status?.empty ? C.amber : C.muted, {
      width: W - 20,
      weight: status ? 'bold' : 'regular'
    });
  }
  function channel(g, side, data) {
    const inst = data.inst || {};
    const typeName = inst.type === 'sample' && inst.mode === 'wavetable'
        ? 'Wavetable'
        : ({ sample: 'Sampler', au: 'Plug-in', midi: 'External MIDI' })[inst.type] || inst.type || '';
    if (side === 'left') {
      g.rect(8, CONTENT_Y + 4, 6, 40, data.track?.color || C.accent, 2);
      heading(g, 22, CONTENT_Y + 3, `Instrument ${pad2(inst.index ?? 0)} · ${typeName}`, 300);
      g.text(22, CONTENT_Y + 19, inst.name || '', 22, C.text, { weight: 'bold', width: W - 30 });
      if (data.assigned === false) chip(g, W - 6, CONTENT_Y + 1, 'NOT ON THIS TRACK', C.amber, 11, { align: 'right', ink: C.black });
      const top = CONTENT_Y + 52, h = KNOBS_Y - 6 - top, x0 = 8, w = W - 16;
      g.rect(x0, top, w, h, C.panel, 5);
      g.text(x0 + 10, top + 6, data.group || '', FONT.small, C.accent, { width: 260, weight: 'bold' });
      const L = x0 + 14, R = x0 + w - 14, T = top + 26, B = top + h - 10;
      if (data.group === 'Envelopes' || data.group === 'Main' && inst.attack != null) {
        const total = Math.max(num(inst.attack) + num(inst.decay) + num(inst.release), .05) * 1.35, span = R - L, hold = span * .26;
        const sx = v => num(v) / total * (span - hold);
        const xa = L + sx(inst.attack), xd = xa + sx(inst.decay), xr = R - sx(inst.release), sus = B - num(inst.sustain) * (B - T);
        g.line(L, B, R, B, C.line);
        g.line(L, B, xa, T, C.accent, 2);
        g.line(xa, T, xd, sus, C.accent, 2);
        g.line(xd, sus, xr, sus, C.accent, 2);
        g.line(xr, sus, R, B, C.accent, 2);
        const ms = v => { const t = num(v); return t >= 1 ? `${t.toFixed(2)} s` : `${Math.round(t * 1000)} ms`; };
        g.text(R, top + 6, `A ${ms(inst.attack)} · D ${ms(inst.decay)} · S ${Math.round(num(inst.sustain) * 100)}% · R ${ms(inst.release)}`, FONT.small, C.muted, {
          align: 'right',
          width: 300
        });
      } else if (data.group === 'Filter · LFO' || data.group === 'Filter') {
        const cut = clamp((Math.log(num(inst.cutoff) || 18000) - Math.log(20)) / (Math.log(20000) - Math.log(20)), 0, 1),
          x = L + cut * (R - L),
          mid = (T + B) / 2,
          res = num(inst.resonance) * (mid - T);
        g.line(L, B, R, B, C.line);
        if (inst.filterType === 'highpass') { g.line(Math.max(L, x - 70), B, x, mid - res, C.accent, 2); g.line(x, mid - res, Math.min(R, x + 16),
            mid, C.accent, 2); g.line(Math.min(R, x + 16), mid, R, mid, C.accent, 2); }
        else if (inst.filterType === 'bandpass') { g.line(Math.max(L, x - 70), B, x, mid - res - 8, C.accent,
            2); g.line(x, mid - res - 8, Math.min(R, x + 70), B, C.accent, 2); }
        else { g.line(L, mid, Math.max(L, x - 16), mid, C.accent, 2); g.line(Math.max(L, x - 16), mid, x, mid - res, C.accent,
            2); g.line(x, mid - res, Math.min(R, x + 70), B, C.accent, 2); }
        g.line(x, T - 4, x, B, C.panel2);
      } else if (data.group === 'Wavetable') {
        // Wavetable: the table's frames with the Position marker (the knob below moves it).
        const peaks = data.sample || [], cols = Math.floor((R - L) / 3), mid = (T + B) / 2, position = clamp(num(inst.wavetablePosition), 0, 1);
        g.line(L, mid, R, mid, C.line);
        for (let c = 0; peaks.length && c < cols; c++) {
          const v = peaks[Math.floor(c / cols * peaks.length)],
            p = Array.isArray(v) ? Math.max(Math.abs(v[0]), Math.abs(v[1])) : Math.abs(num(v));
          const bh = Math.max(1, p * (B - T) / 2);
          g.rect(L + c * 3, mid - bh, 2, bh * 2, C.muted);
        }
        const x = L + position * (R - L);
        g.rect(Math.max(L, x - 2), T - 4, 4, B - T + 8, C.accent, 1);
        g.text(R, top + 6, `Position ${Math.round(position * 100)}%`, FONT.small, C.accent, { align: 'right', width: 200, weight: 'bold' });
      } else if (data.sample?.length) {
        const peaks = data.sample, cols = Math.floor((R - L) / 3), mid = (T + B) / 2;
        g.line(L, mid, R, mid, C.line);
        for (let c = 0; c < cols; c++) {
          const v = peaks[Math.floor(c / cols * peaks.length)],
            p = Array.isArray(v) ? Math.max(Math.abs(v[0]), Math.abs(v[1])) : Math.abs(num(v));
          const bh = Math.max(1, p * (B - T) / 2);
          g.rect(L + c * 3, mid - bh, 2, bh * 2, C.muted);
        }
      } else if (/LFO/.test(data.group || '')) {
        const mid = (T + B) / 2, amp = (B - T) / 2 - 4; let px = L, py = mid;
        g.line(L, mid, R, mid, C.line);
        for (let i = 1; i <= 96; i++) {
          const x = L + (R - L) * i / 96,
            y = mid - Math.sin(i / 96 * Math.PI * 6) * amp;
          g.line(px, py, x, y, C.accent, 2);
          px = x;
          py = y;
        }
      } else g.text(W / 2, (T + B) / 2 - 10, data.group || typeName, 16, C.dim, { align: 'center', width: W - 40 });
      return;
    }
    if (data.instruments) { drawList(g, data.instruments); return; }
    heading(g, 10, CONTENT_Y + 3, 'Pages', 200);
    const pages = data.pages || [];
    pages.slice(0, LIST_ROWS).forEach((name, i) => {
      const y = CONTENT_Y + 19 + i * 20,
        on = i === data.page;
      if (on) g.rect(4, y, 210, 19, C.accent, 4);
      g.text(14, y + 2, name, 14, on ? C.black : C.text, { width: 190, weight: on ? 'bold' : 'regular' });
    });
    if (!pages.length) g.text(10, CONTENT_Y + 22, data.group || 'Track channel', 14, C.muted, { width: 210 });
    heading(g, 234, CONTENT_Y + 3, 'Track', 200);
    swatch(g, 234, CONTENT_Y + 23, data.track?.color, 10);
    g.text(250, CONTENT_Y + 19, data.track?.name || '', 15, C.text, { width: 222, weight: 'bold' });
    g.text(234, CONTENT_Y + 48, '◄ ►: pages · display 1–3: level', FONT.small, C.hint, { width: 240 });
    g.text(234, CONTENT_Y + 66, 'Main encoder: instrument', FONT.small, C.hint, { width: 240 });
    g.text(234, CONTENT_Y + 84, 'Push: audition', FONT.small, C.hint, { width: 240 });
  }
  function plugin(g, side, data) {
    if (side === 'left') {
      if (data.chain) { drawList(g, data.chain); return; }
      heading(g, 10, CONTENT_Y + 3, `${data.slotLabel || ''}${data.nks ? ' · NKS' : ''}`, 300);
      g.text(10, CONTENT_Y + 19, data.name || '', 22, C.text, { weight: 'bold', width: W - 20 });
      return;
    }
    heading(g, 10, CONTENT_Y + 3, `${data.slotLabel || ''}${data.nks ? ' · NKS' : ''}`, W - 20, C.accent);
    g.text(10, CONTENT_Y + 18, data.name || '', 20, C.text, { weight: 'bold', width: W - 20 });
    const pages = data.pages || [], page = clamp(num(data.page), 0, Math.max(0, pages.length - 1));
    statusLine(g, CONTENT_Y + 44, data.status || (data.error ? {
      text: data.error,
      error: true
    } : null), pages.length ? `Page ${page + 1} / ${pages.length} · ${data.focus === 'pages' ? 'turn: pages · push: chain' : 'push: pages'}` : '');
    if (data.status && !pages.length) { g.text(W / 2, CONTENT_Y + 96,
        data.status.loading ? 'Waiting for the plug-in…' : data.status.error ? 'Plug-in unavailable' : 'Nothing to control here', 16, C.dim,
        { align: 'center', width: W - 40 }); return; }
    // Four rows: the page list ends above the encoder labels (five ran into them).
    drawList(g,
        data.pageList
            ? { ...data.pageList, title: 'Pages', rows: PAGE_ROWS, top: windowTop(pages.length, page, PAGE_ROWS) }
            : { title: 'Pages', items: pages.map((name, i) => ({ name: `${i + 1}  ${name}` })), selected: page, rows: PAGE_ROWS },
        { y: CONTENT_Y + 60 });
  }
  function arranger(g, side, data) {
    const blocks = data.blocks || [], selected = data.selected || 0;
    if (side === 'left') {
      heading(g, 8, CONTENT_Y + 3, `Song · ${blocks.length} blocks · ${data.slots} slots`, 300);
      if (blocks.length && !data.loop) g.text(W - 8, CONTENT_Y + 2, `${selected + 1} / ${blocks.length}`, FONT.small, C.dim, {
        align: 'right',
        width: 80
      });
      if (data.loop) chip(g, W - 8, CONTENT_Y + 1, 'SECTION LOOP', C.amber, 11, { align: 'right', ink: C.black });
      const rows = LIST_ROWS,
        rowH = 19,
        start = clamp(data.top != null ? num(data.top) : windowTop(blocks.length, selected, rows), 0, Math.max(0, blocks.length - rows)),
        y0 = CONTENT_Y + 20;
      if (blocks.length > rows) {
        const track = rows * rowH - 2,
          thumb = Math.max(8, track * rows / blocks.length),
          pos = (track - thumb) * start / Math.max(1, blocks.length - rows);
        g.rect(W - 3, y0, 3, track, C.panel2, 1);
        g.rect(W - 3, y0 + pos, 3, thumb, C.muted, 1);
      }
      for (let i = 0; i < Math.min(rows, blocks.length - start); i++) {
        const b = blocks[start + i], y = y0 + i * rowH, on = start + i === selected;
        if (b.playing) g.rect(0, y, W, rowH - 2, C.play);
        if (on) g.rect(4, y, W - 12, rowH - 2, C.accent, 4);
        if (b.loop) g.rect(0, y, 3, rowH - 2, C.amber);
        const fg = on ? C.black : C.text;
        g.text(32, y + 2, pad2(b.index + 1), FONT.mono, on ? C.black : C.dim, { align: 'right', mono: true, width: 24 });
        swatch(g, 40, y + 4, b.color, 10);
        g.text(56, y + 2, `P${pad2(b.number)}`, FONT.mono, fg, { mono: true, width: 34, weight: 'bold' });
        g.text(96, y + 2, b.name, 14, fg, { width: 250, weight: on ? 'bold' : 'regular' });
        g.text(W - 62, y + 2, b.progress || `×${b.count}`, 14, on ? C.black : b.progress ? C.green : b.count > 1 ? C.amber : C.dim, {
          align: 'right',
          width: 50,
          weight: 'bold'
        });
        g.text(W - 14, y + 3, `${b.length}`, FONT.small, on ? C.black : C.dim, { align: 'right', width: 40 });
      }
      if (!blocks.length) g.text(W / 2, CONTENT_Y + 64, 'The song is empty · Insert adds a block', 15, C.dim, { align: 'center', width: W - 40 });
      return;
    }
    const b = blocks[selected];
    if (b) {
      g.rect(10, CONTENT_Y + 8, 6, 64, b.color, 2);
      heading(g, 24, CONTENT_Y + 6, `Block ${b.index + 1} · pattern ${pad2(b.number)}`, 300);
      g.text(24, CONTENT_Y + 22, b.name, 22, C.text, { weight: 'bold', width: W - 34 });
      g.text(24, CONTENT_Y + 54, `×${b.count} · ${b.length} rows${b.loop ? ' · in loop' : ''}${b.playing ? ' · playing ' + b.progress : ''}`, 15, b.playing ? C.green : C.text, {
        width: W - 34
      });
    }
    g.text(10, CONTENT_Y + 88, `Playback: ${data.mode === 'song' ? 'Song' : 'Pattern'}  ·  Song loop ${data.loopSong ? 'on' : 'off'}`, FONT.body, C.muted, {
      width: W - 20
    });
    hints(g, ['Main encoder: block · push: edit pattern · tilt ◄ ►: pattern', 'SCENE pads jump to blocks · Shift + pads loop them'], CONTENT_Y + 112);
  }
  const db = v => {
    if (v <= .0001) return '−∞ dB';
    const r = Math.round(20 * Math.log10(v) * 10) / 10;
    return `${r > 0 ? '+' : r < 0 ? '−' : ''}${Math.abs(r).toFixed(1)} dB`;
  };
  const pan = v => Math.abs(v) < .005 ? 'C' : `${Math.round(Math.abs(v) * 100)}${v < 0 ? 'L' : 'R'}`;
  function mixer(g, side, data) {
    const strips = (data.strips || []).filter(s => side === 'left' ? s.index % 8 < 4 : s.index % 8 >= 4);
    const param = data.param;
    const contextH = 0;
    strips.forEach(s => {
      const x = (s.index % 4) * COL, top = CONTENT_Y + 2 + contextH, h = CONTENT_H - 4 - contextH;
      if (s.selected) g.rect(x + 3, top, COL - 6, h, C.panel, 6);
      g.rect(x + 10, top + 4, COL - 20, 3, s.mute ? C.dim : s.color, 1);
      g.text(x + COL / 2, top + 11, s.name, FONT.body, s.mute ? C.dim : C.text, {
        align: 'center',
        width: COL - 14,
        weight: s.selected ? 'bold' : 'regular'
      });
      const fy = top + 34, fh = h - 66, value = param === 'volume' ? s.volume / 1.5 : param === 'pan' ? (s.pan + 1) / 2 : s.value;
      // Meter (left) and fader / pan / send (right).
      const peak = clamp(num(s.peak), 0, 1);
      g.rect(x + 26, fy, 6, fh, C.panel2, 2);
      if (peak > 0) g.rect(x + 26, fy + fh * (1 - peak), 6, fh * peak, peak >= .98 ? C.red : peak > .8 ? C.amber : C.green, 1);
      if (param === 'pan') {
        const mid = fy + fh / 2, px = x + 44 + (s.pan + 1) / 2 * 52;
        g.rect(x + 44, mid - 1, 56, 2, C.line); g.rect(x + 71, mid - 7, 2, 14, C.dim);
        g.rect(px - 3, mid - 10, 6, 20, s.mute ? C.dim : C.accent, 2);
      } else {
        g.rect(x + 46, fy, 20, fh, C.panel2, 3);
        const vh = fh * clamp(num(value), 0, 1); if (vh > 0) g.rect(x + 46, fy + fh - vh, 20, vh, s.mute ? C.dim : mix(C.bg, s.color, .85), 3);
        if (param === 'volume') { const unity = fy + fh * (1 - 1 / 1.5); g.line(x + 42, unity, x + 70, unity, C.muted); }
      }
      if (s.mute) chip(g, x + 80, fy + 2, 'M', C.red, 12, { ink: C.white });
      if (s.solo) chip(g, x + 80, fy + 26, 'S', C.amber, 12, { ink: C.black });
      const label = param === 'volume' ? db(s.volume) : param === 'pan' ? pan(s.pan) : `${Math.round(value * 100)}%`;
      g.text(x + COL / 2, fy + fh + 8, label, FONT.body, C.text, { align: 'center', width: COL - 14, weight: 'bold' });
    });
    if (!strips.length) g.text(W / 2, CONTENT_Y + 60 + contextH, 'No tracks in this bank', 15, C.dim, {
      align: 'center',
      width: W - 40
    });
    if (side === 'right') {
      if (strips.length < 4) {
        const x = strips.length * COL + 10;
        heading(g, x, CONTENT_Y + 6 + contextH, 'Master', 100);
        g.text(x, CONTENT_Y + 24 + contextH, db(data.master), 18, C.text, { width: COL * (4 - strips.length) - 20, weight: 'bold' });
      }
    }
  }
  function browser(g, side, data) {
    if (side === 'left') {
      if (data.filterList) { drawList(g, data.filterList); return; }
      if (data.overview) {
        const end = drawList(g, data.overview);
        if (data.overview.items?.some(item => item.filterId)) g.text(10, Math.min(end + 2,
            CONTENT_Y + CONTENT_H - 16), 'Tilt ◄ or turn a filter encoder: open its list', FONT.small, C.hint, {
          width: W - 20
        });
        return;
      }
    }
    const results = data.results;
    if (!results) return;
    const end = drawList(g, results);
    const item = results.items?.[results.selected], l = data.list || {};
    const footer = item ? item.detail || '' : '';
    g.line(8, end + 1, W - 8, end + 1, C.line);
    g.text(10, end + 4, `${l.preview ? '▶ ' : ''}${footer || (results.active ? 'Push: load · Shift + push: load new' : 'Tilt ►: results')}`, FONT.small, l.preview ? C.accent : C.hint, {
      width: W - 20
    });
  }
  function sample(g, side, data) {
    const info = data.info; if (!info) return;
    const cols = info.columns, top = CONTENT_Y, h = 92, mid = top + h / 2;
    const view = info.view || [0, 1], span = Math.max(1e-9, view[1] - view[0]), offset = side === 'left' ? 0 : W;
    const px = pos => (pos - view[0]) / span * W * 2 - offset;
    g.rect(0, top, W, h, C.panel);
    const r0 = px(info.start), r1 = px(info.end);
    g.rect(r0, top, r1 - r0, h, '#2a2219');
    if (info.loop) g.rect(px(info.loopStart), top + h - 8, px(info.loopEnd) - px(info.loopStart), 8, '#5a4a26');
    g.line(0, mid, W, mid, C.line);
    if (cols?.max?.length) {
      const total = cols.max.length, step = 2;
      for (let x = 0; x < W; x += step) {
        const c = Math.floor((x + offset) / (W * 2) * total), max = clamp(num(cols.max[c]), -1, 1), min = clamp(num(cols.min[c]), -1, 1);
        const y1 = mid - max * (h / 2 - 4), y2 = mid - min * (h / 2 - 4), inside = x + offset >= r0 + offset && x + offset <= r1 + offset;
        g.rect(x, Math.min(y1, y2), 1, Math.max(1, Math.abs(y2 - y1)), cols.approximate ? '#8f8768' : inside ? '#d9cfab' : '#7d7660');
      }
    } else if (side === 'left') g.text(W / 2, mid - 9, info.path ? 'Reading waveform…' : 'Load a sample to edit it', 15, C.dim,
        { align: 'center', width: W - 40 });
    for (const m of info.markers || []) {
      const x = px(m.pos); if (x < -2 || x > W + 2) continue;
      const on = m.key === info.marker, color = on ? C.white : m.key === 'start' || m.key === 'end' ? C.accent : m.slice ? '#b9965a' : C.amber;
      g.line(x, top, x, top + h, color, on ? 2 : 1);
      if (on || !m.slice) {
        const label = on ? m.label : m.label.slice(0, 1);
        const lw = textWidth(label, 11, true) + 8;
        const lx = clamp(x + 2, 0, W - lw);
        g.rect(lx, top + 2, lw, 15, on ? C.white : C.bg, 3);
        g.text(lx + 4, top + 3, label, 11, on ? C.black : color, { width: lw - 6, weight: 'bold' });
      }
    }
    const y = top + h + 7;
    if (side === 'left') {
      g.text(10, y, info.name || '', 16, C.text, { weight: 'bold', width: W - 20 });
      g.text(10, y + 21, `${info.markerLabel} · ${info.time}${info.snap ? ' · zero-snap' : ''}`, 14, C.accent, { width: W - 20, weight: 'bold' });
      g.text(10, y + 40, info.message || 'Main encoder nudges the marker · push auditions', FONT.small, info.message ? C.text : C.hint, {
        width: W - 20
      });
    } else {
      g.text(10, y, `${info.modeLabel} · root ${noteLabel(Math.round(info.rootNote))}${info.mode === 'slice' ? ` · ${info.slices} slices` : ''}`, 15, C.text, {
        width: W - 20,
        weight: 'bold'
      });
      g.text(10, y + 21, `${info.seconds ? info.seconds.toFixed(2) + ' s · ' : ''}view ${Math.round(view[0] * 100)}–${Math.round(view[1] * 100)}%`, FONT.body, C.muted, {
        width: W - 20
      });
      g.text(10, y + 40, data.recording
          ? '● Recording audio input'
          : info.busy
          ? 'Editing…'
          : 'Edits write a new file · Shift + SAMPLING records', FONT.small, data.recording ? C.red : C.hint, {
        width: W - 20,
        weight: data.recording ? 'bold' : 'regular'
      });
    }
  }
  function info(g, side, data) {
    const lines = data.lines || [];
    if (side === 'left') {
      g.text(12, CONTENT_Y + 8, data.title || '', 22, C.text, { weight: 'bold', width: W - 24 });
      paragraphs(g, 12, CONTENT_Y + 44, lines.slice(0, 3), 14, W - 24, [C.text, C.muted], 20);
      return;
    }
    paragraphs(g, 12, CONTENT_Y + 10, lines.slice(3, 10), 14, W - 24, [C.muted, C.muted], 20);
  }
  function overlay(g, side, data, model) {
    const lines = data.lines || [];
    if (side === 'left') {
      // Title and the big value, centred; hints go to the right screen.
      heading(g, W / 2, CONTENT_Y + 26, model.page.title, 400, C.accent, 'center');
      const big = String(data.big || ''), size = big.length > 18 ? 26 : big.length > 12 ? 32 : 40;
      g.text(W / 2, CONTENT_Y + 48 + (40 - size) / 2, big, size, C.text, { align: 'center', width: W - 24, weight: 'bold' });
      if (data.chords?.length && lines[1]) g.text(W / 2, CONTENT_Y + 104, lines[1], 14, C.muted, { align: 'center', width: W - 24 });
      return;
    }
    if (data.chords?.length) {
      // Laid out like the pads: chord 1 bottom-left, 13 top-left.
      heading(g, 10, CONTENT_Y + 3, 'Chord pads', 200);
      chordPads(g, data.chords);
      return;
    }
    paragraphs(g, 12, CONTENT_Y + 10, lines.slice(0, 7), 14, W - 24, [C.text, C.muted], 20);
  }
  function stepView(g, side, data) {
    const gd = data.grid; if (!gd) return;
    if (side === 'left') {
      heading(g, 8, CONTENT_Y + 3, data.title || 'Steps', W - 16);
      gd.cells.forEach((cell, i) => {
        const x = 4 + i % 8 * 59,
          y = CONTENT_Y + 20 + Math.floor(i / 8) * 67,
          held = data.held?.includes(cell.row),
          sel = data.events?.includes(cell.row);
        const bg = cell.outside ? C.bg : held ? C.accent : sel ? '#5a4a22' : cell.playhead ? C.play : cell.note ? tint(gd.color, .42) : C.panel;
        g.rect(x, y, 55, 62, bg, 5);
        if (cell.cursor) g.frame(x, y, 55, 62, C.white, 2);
        if (cell.outside) return;
        g.text(x + 5, y + 4, pad2(cell.row), 11, held ? C.black : (cell.row % 4 === 0 ? C.muted : C.dim), { mono: true, width: 22 });
        g.text(x + 27.5, y + 22, cell.note || '', 15, held ? C.black : C.text, { align: 'center', width: 52, weight: 'bold' });
        if (cell.velocity) valueBar(g, x + 6, y + 50, 43, 4, cell.velocity / 127, held ? C.black : gd.color);
      });
      return;
    }
    const s = data.step;
    if (s) {
      heading(g, 10, CONTENT_Y + 3, data.title || 'Step', 200);
      g.text(10, CONTENT_Y + 18, noteLabel(s.note), 36, C.text, { weight: 'bold', width: 150 });
      [['Velocity', String(s.velocity ?? 100)], ['Gate', gateLabel(s, ' rows')], ['FX 1', fxLabel(s.fx1)],
          ['FX 2', fxLabel(s.fx2)]].forEach(([label, value], i) => {
        const y = CONTENT_Y + 6 + i * 19;
        g.text(176, y + 1, label, FONT.small, C.muted, { width: 70 });
        g.text(250, y, value, FONT.body, C.text, { width: 220, weight: 'bold' });
      });
    }
    hints(g, data.lines || ['Hold step pads and turn encoders to edit them', 'Tap a lit pad to remove its step'], CONTENT_Y + (s ? 90 : 10));
  }
  /** CHANNEL · Track / Master levels: the channel and its values (left); the level breadcrumb (right). */
  function levels(g, side, data) {
    if (side === 'left') {
      g.rect(8, CONTENT_Y + 4, 6, 46, isHex(data.color) ? data.color : C.accent, 2);
      heading(g, 22, CONTENT_Y + 3, data.subtitle || '', W - 30);
      g.text(22, CONTENT_Y + 19, data.title || '', 22, C.text, { weight: 'bold', width: W - 30 });
      paragraphs(g, 22, CONTENT_Y + 62, data.lines || [], FONT.body, W - 32, [C.text, C.muted], 20);
      return;
    }
    const list = data.levels || [['master', 'Master'], ['track', 'Track'], ['instrument', 'Instr.']];
    heading(g, 10, CONTENT_Y + 3, `Level · display buttons 1–${list.length}`, W - 20);
    let x = 10;
    list.forEach(([key, label], i) => {
      if (i) { g.text(x, CONTENT_Y + 22, '›', 15, C.dim, { width: 12 }); x += 16; }
      x += chip(g, x, CONTENT_Y + 21, label, key === data.level ? C.accent : C.raised, FONT.small, {
        ink: key === data.level ? C.black : C.muted
      }) + 8;
    });
    hints(g, [data.level === 'group' ? 'Group bus: volume · pan · mute · solo' : 'Instrument parameters: PLUG-IN · Instr.', 'Inserts of this level: PLUG-IN',
      data.level === 'group' ? '4-D tilt ◄ ►: tracks of the group' : '◄ ►: pages · Shift + ◄ ►: group bank'], CONTENT_Y + 52);
  }
  const DRAW = { grid, list, channel, plugin, arranger, mixer, browser, sample, info, overlay, step: stepView, levels };

  function renderSide(model, side) {
    const g = canvas();
    g.clear(C.bg);
    const offset = side === 'left' ? 0 : 4;
    drawButtons(g, model.buttons.slice(offset, offset + 4));
    drawTitle(g, side, model.title);
    drawProgress(g, side, model.title.progress);
    const data = model.page.data || {}, draw = DRAW[data.kind] || info;
    try {
      draw(g, side, data, model);
    } catch (error) {
      g.text(10, CONTENT_Y + 10, 'Screen error: ' + (error.message || error), FONT.small, C.red, { width: W - 20 });
    }
    if (model.title.countIn && side === 'left') {
      g.rect(150, CONTENT_Y + 14, 180, 122, C.red, 10);
      g.text(240, CONTENT_Y + 22, 'COUNT-IN', 12, C.white, { align: 'center', width: 160, weight: 'bold' });
      g.text(240, CONTENT_Y + 40, String(model.title.countIn), 64, C.white, { align: 'center', width: 160, weight: 'bold' });
    }
    const header = model.page.data?.header;
    if (header) drawHeader(g, side, header);
    if (model.touch && (model.touch.index < 4) === (side === 'left')) drawTouch(g, model);
    if (model.flash && side === 'left' && !model.touch) drawFlash(g, model.flash, header ? HEADER_H + 2 : 0);
    drawKnobs(g, model.knobs, offset, model);
    if (model.brightness != null
        && model.brightness < .999) for (const op of g.ops) for (const key of ['color', 'background']) if (op[key]) op[key] = scaleColor(op[key], model.brightness);
    return g.ops;
  }
  function render(model) { return { left: renderSide(model, 'left'), right: renderSide(model, 'right') }; }

  window.STMK3Screens = { render, WIDTH: W, HEIGHT: H, colors: C, LIST_ROWS };
})();
