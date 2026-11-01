'use strict';
// A small DOM with layout boxes for keyboard-navigation tests: elements carry a rect, selectors support
// tags, #id, .class, [attr], [attr="v"], [attr^="v"], :not(), descendant and child combinators.

function simpleMatches(node, selector) {
  let rejected = false;
  selector = selector.replace(/:not\(([^()]*)\)/g, (_, part) => {
    if (part.split(',').some(item => simpleMatches(node, item.trim()))) rejected = true;
    return '';
  });
  if (rejected) return false;
  if (selector.includes(':disabled')) {
    if (!node.disabled) return false;
    selector = selector.replace(':disabled', '');
  }
  let ok = true;
  selector = selector.replace(/\[([^\]=^]+)(\^?=)?["']?([^\]"']*)["']?\]/g, (_, name, op, value) => {
    const actual = node.getAttribute(name);
    if (actual === null || (op === '=' && actual !== value) || (op === '^=' && !actual.startsWith(value))) ok = false;
    return '';
  });
  if (!ok) return false;
  const id = selector.match(/#([\w-]+)/)?.[1];
  if (id && node.id !== id) return false;
  if ([...selector.matchAll(/\.([\w-]+)/g)].some(match => !node.classes.has(match[1]))) return false;
  const tag = selector.match(/^[\w-]+/)?.[0];
  return !tag || node.tagName === tag.toUpperCase();
}
function splitList(selector) {
  const parts = []; let depth = 0, current = '';
  for (const char of selector) {
    if (char === '(' || char === '[') depth++;
    if (char === ')' || char === ']') depth--;
    if (char === ',' && !depth) { parts.push(current); current = ''; } else current += char;
  }
  return [...parts, current].map(part => part.trim()).filter(Boolean);
}
function matches(node, selector) {
  return splitList(selector).some(part => {
    const tokens = part.replace(/\s*>\s*/g, ' > ').split(/\s+(?![^[]*\])/);
    let current = node, index = tokens.length - 1;
    if (!simpleMatches(current, tokens[index--])) return false;
    while (index >= 0) {
      const direct = tokens[index] === '>';
      if (direct) index--;
      current = current.parentElement;
      if (!direct) while (current && current.tagName !== '#DOCUMENT' && !simpleMatches(current, tokens[index])) current = current.parentElement;
      if (!current || current.tagName === '#DOCUMENT' || !simpleMatches(current, tokens[index--])) return false;
    }
    return true;
  });
}

function createDOM() {
  const listeners = { window: {}, document: {} };
  const document = { activeElement: null, documentElement: { clientWidth: 1500, clientHeight: 940 } };
  const nodes = new Map();
  const descendants = root => root.children.flatMap(child => [child, ...descendants(child)]);
  const CONTROL = ['BUTTON', 'INPUT', 'SELECT', 'TEXTAREA', 'SUMMARY'];
  function fire(target, type, event = {}) {
    for (const listener of listeners.document[type] || []) listener({ target, ...event });
  }
  function el(tag, options = {}, parent = document.body) {
    const node = {
      tagName: tag.toUpperCase(), id: options.id || '', classes: new Set((options.className || '').split(' ').filter(Boolean)),
      attrs: { ...(options.attrs || {}) }, children: [], parentElement: parent, isConnected: true, hidden: !!options.hidden,
      disabled: !!options.disabled, type: options.type || '', value: options.value || '', rect: options.rect || null,
      dataset: {}, style: {}, scrolls: 0,
      get className() { return [...this.classes].join(' '); }, set className(value) { this.classes = new Set(String(value).split(' ').filter(Boolean)); },
      classList: null,
      get tabIndex() { return this.attrs.tabindex !== undefined ? Number(this.attrs.tabindex) : CONTROL.includes(this.tagName) ? 0 : -1; },
      set tabIndex(value) { this.attrs.tabindex = String(value); },
      getAttribute(name) {
        if (name === 'id') return this.id || null;
        if (name === 'type') return this.type || null;
        if (name === 'class') return this.className || null;
        return Object.hasOwn(this.attrs, name) ? String(this.attrs[name]) : null;
      },
      setAttribute(name, value) { if (name === 'id') this.id = value; else this.attrs[name] = String(value); },
      hasAttribute(name) { return this.getAttribute(name) !== null; },
      removeAttribute(name) { delete this.attrs[name]; },
      matches(selector) { return matches(this, selector); },
      closest(selector) { for (let current = this; current && current.tagName !== '#DOCUMENT'; current = current.parentElement) if (current.matches(selector)) return current; return null; },
      contains(other) { for (let current = other; current; current = current.parentElement) if (current === this) return true; return false; },
      querySelectorAll(selector) { return descendants(this).filter(child => child.isConnected && child.matches(selector)); },
      querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
      getClientRects() {
        if (!this.isConnected) return [];
        for (let current = this; current && current.tagName !== '#DOCUMENT'; current = current.parentElement) if (current.hidden) return [];
        return [{}];
      },
      getBoundingClientRect() {
        const r = this.rect || { left: 0, top: 0, width: 0, height: 0 };
        return { left: r.left, top: r.top, width: r.width, height: r.height, right: r.left + r.width, bottom: r.top + r.height };
      },
      focus() {
        if (!this.isConnected || this.disabled || (this.tabIndex < 0 && this.attrs.tabindex === undefined)) return;
        document.activeElement = this; fire(this, 'focusin');
      },
      blur() { if (document.activeElement === this) document.activeElement = document.body; },
      scrollIntoView() { this.scrolls++; },
      addEventListener() {}, append(...children) { this.children.push(...children); }, appendChild(child) { this.children.push(child); child.parentElement = this; return child; },
      remove() { this.isConnected = false; },
      get textContent() { return this._text || ''; }, set textContent(value) { this._text = value; },
      set innerHTML(value) { this.children = []; }
    };
    node.classList = { add: c => node.classes.add(c), remove: c => node.classes.delete(c), toggle: (c, on) => (on ?? !node.classes.has(c)) ? node.classes.add(c) : node.classes.delete(c), contains: c => node.classes.has(c) };
    for (const [key, value] of Object.entries(node.attrs)) if (key.startsWith('data-')) node.dataset[key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = String(value);
    if (parent) parent.children.push(node);
    if (node.id) nodes.set(node.id, node);
    return node;
  }
  const root = { tagName: '#DOCUMENT', children: [], parentElement: null };
  document.body = el('body', { rect: { left: 0, top: 0, width: 1500, height: 940 } }, null);
  document.body.parentElement = root;
  document.activeElement = document.body;
  document.querySelectorAll = selector => [document.body, ...document.body.querySelectorAll(selector)].filter(node => node !== document.body || node.matches(selector));
  document.querySelector = selector => document.querySelectorAll(selector)[0] || null;
  document.getElementById = id => nodes.get(id)?.isConnected ? nodes.get(id) : null;
  document.addEventListener = (type, listener) => (listeners.document[type] ||= []).push(listener);
  document.createElement = tag => el(tag, {}, null);
  const window = { innerWidth: 1500, innerHeight: 940, addEventListener: (type, listener) => (listeners.window[type] ||= []).push(listener) };
  function clear() {
    for (const node of descendants(document.body)) node.isConnected = false;
    document.body.children = []; document.activeElement = document.body; nodes.clear();
  }
  const box = (left, top, width, height) => ({ left, top, width, height });
  return { document, window, el, clear, box, nodes, listeners, matches };
}

module.exports = { createDOM, matches };
