'use strict';
// The small DOM of insert_chain_ui_tests.js (HTML parsing and selector matching for rendered markup), shared by
// instrument_effects_ui_tests.js.
// ---- A small DOM: enough HTML parsing and selector matching for the chain's own markup -------------------------
const decode = value => value.replace(/&(amp|lt|gt|quot|#39);/g, (_, e) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" }[e]));
const VOID = new Set(['INPUT', 'BR', 'IMG', 'PATH', 'CIRCLE', 'LINE']);
function compound(node, part) {
  if (!node || !node.attributes) return false;
  for (const token of part.match(/#[\w-]+|\.[\w-]+|\[[^\]]+\]|^[a-z][\w-]*/gi) || []) {
    if (token[0] === '#') { if (node.id !== token.slice(1)) return false; }
    else if (token[0] === '.') { if (!node.classes.has(token.slice(1))) return false; }
    else if (token[0] === '[') { const [, name, value] = /^\[([^=\]]+)(?:="([^"]*)")?\]$/.exec(token); if (!Object.hasOwn(node.attributes, name) || (value !== undefined && node.attributes[name] !== value)) return false; }
    else if (node.tagName !== token.toUpperCase()) return false;
  }
  return true;
}
function matches(node, selector) {
  const parts = selector.trim().split(/\s*(>)\s*|\s+/).filter(Boolean);
  const walk = (item, index) => {
    if (!compound(item, parts[index])) return false;
    if (index === 0) return true;
    if (parts[index - 1] === '>') return walk(item.parentElement, index - 2);
    for (let parent = item.parentElement; parent; parent = parent.parentElement) if (walk(parent, index - 1)) return true;
    return false;
  };
  return walk(node, parts.length - 1);
}
function makeDocument() {
  let all = [];
  const stubs = new Map();
  const stub = id => { if (!stubs.has(id)) stubs.set(id, { id, innerHTML: '', value: '', dataset: {}, style: {}, classList: { toggle() {}, add() {}, remove() {} }, focus() { doc.activeElement = this; }, select() {}, scrollIntoView() {}, closest() { return null; }, querySelector() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, removeEventListener() {}, setAttribute() {}, getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 20 }), isConnected: false }); return stubs.get(id); };
  const doc = { activeElement: null, body: stub('body'), readyState: 'complete', addEventListener() {}, removeEventListener() {}, createElement: tag => stub('new-' + tag + Math.random()),
    getElementById: id => all.find(node => node.id === id) || stub(id),
    querySelector: selector => all.find(node => matches(node, selector)) || (/^#[\w-]+$/.test(selector) ? stub(selector.slice(1)) : null),
    querySelectorAll: selector => all.filter(node => matches(node, selector)),
    mount(markup) {
      for (const node of all) node.isConnected = false;
      all = []; const stack = [];
      for (const token of markup.matchAll(/<\/?[\w:-]+\b[^>]*>|[^<]+/g)) {
        const raw = token[0];
        if (!raw.startsWith('<')) { if (stack.length) stack.at(-1).textContent += decode(raw); continue; }
        const tagName = /^<\/?([\w:-]+)/.exec(raw)[1].toUpperCase();
        if (raw.startsWith('</')) { while (stack.length && stack.pop().tagName !== tagName) {} continue; }
        const attributes = {}, dataset = {};
        for (const m of raw.slice(/^<[\w:-]+/.exec(raw)[0].length, -1).matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'))?/g)) {
          const value = decode(m[2] ?? m[3] ?? ''); attributes[m[1]] = value;
          if (m[1].startsWith('data-')) dataset[m[1].slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
        }
        const node = { id: attributes.id || '', tagName, attributes, dataset, parentElement: stack.at(-1) || null, classes: new Set((attributes.class || '').split(/\s+/).filter(Boolean)), handlers: {}, isConnected: true, ownerDocument: doc,
          textContent: '', value: attributes.value || '', disabled: Object.hasOwn(attributes, 'disabled'), style: {}, selectionStart: 0,
          get open() { return Object.hasOwn(this.attributes, 'open'); },
          classList: { add: (...c) => c.forEach(x => node.classes.add(x)), remove: (...c) => c.forEach(x => node.classes.delete(x)), contains: c => node.classes.has(c), toggle() {} },
          setAttribute(name, value) { this.attributes[name] = String(value); }, getAttribute(name) { return this.attributes[name] ?? null; },
          addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }, removeEventListener(type, fn) { this.handlers[type] = (this.handlers[type] || []).filter(x => x !== fn); },
          focus() { doc.activeElement = this; }, setPointerCapture() {}, setSelectionRange() {}, scrollIntoView() {},
          closest(selector) { for (let item = this; item; item = item.parentElement) if (matches(item, selector)) return item; return null; },
          querySelector(selector) { return all.find(item => item !== this && matches(item, selector) && contains(this, item)) || null; },
          querySelectorAll(selector) { return all.filter(item => item !== this && matches(item, selector) && contains(this, item)); },
          click() { return this.fire('click'); },
          fire(type, extra = {}) { const event = { type, target: this, button: 0, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; let result; for (const fn of [...(this.handlers[type] || [])]) result = fn(event); return Promise.resolve(result).then(() => event); },
          dispatchEvent(event) { return this.fire(event.type, event); } };
        if (stack.length) stack.at(-1).textContent;
        all.push(node);
        if (!VOID.has(tagName) && !raw.endsWith('/>')) stack.push(node);
      }
    } };
  const contains = (parent, item) => { for (let p = item.parentElement; p; p = p.parentElement) if (p === parent) return true; return false; };
  return doc;
}

module.exports = { makeDocument, matches };
