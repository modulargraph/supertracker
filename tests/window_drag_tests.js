'use strict';
// window-drag.js: only empty top bar space drags the window; controls keep their clicks.
const assert = require('node:assert'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');

class Element {
  constructor(tag, parent = null, attrs = {}, cursor = 'auto') { Object.assign(this, { tag, parent, attrs, cursor }); }
  matches(selector) {
    return selector.split(',').some(part => {
      part = part.trim();
      if (part.startsWith('.')) return (this.attrs.class || '').split(' ').includes(part.slice(1));
      const attr = part.match(/^\[([a-z-]+)(?:="([^"]+)")?\]$/);
      if (attr) return attr[1] in this.attrs && (attr[2] === undefined || this.attrs[attr[1]] === attr[2]);
      return this.tag === part;
    });
  }
  closest(selector) { for (let node = this; node; node = node.parent) if (node.matches(selector)) return node; return null; }
}
const listeners = {}, posted = [];
const context = {
  Element, getComputedStyle: element => ({ cursor: element.cursor }),
  document: { addEventListener: (type, fn) => (listeners[type] = fn), documentElement: { addEventListener: (type, fn) => (listeners['doc:' + type] = fn) } },
};
context.window = { ...context, addEventListener: (type, fn) => (listeners['win:' + type] = fn), webkit: { messageHandlers: { windowDrag: { postMessage: value => posted.push(value) } } } };
vm.createContext(context);
vm.runInContext(fs.readFileSync(path.join(__dirname, '../Resources/Web/window-drag.js'), 'utf8'), context);

const body = new Element('main'), bar = new Element('header', body, { class: 'topbar' }), brand = new Element('div', bar, { class: 'brand' });
const button = new Element('button', bar), tempo = new Element('input', bar), clickable = new Element('div', bar, {}, 'pointer');
const { draggable } = context.window.STWindowDrag;
assert.equal(draggable(bar), true); assert.equal(draggable(brand), true);
for (const control of [button, tempo, clickable, body]) assert.equal(draggable(control), false);
console.log('PASS empty top bar space drags; controls and the rest of the page do not');

listeners.mouseover({ target: brand }); listeners.mousemove({ target: brand, buttons: 0 }); listeners.mousemove({ target: button, buttons: 1 });
assert.deepEqual(posted, [true], 'reports only changes, and not while a button is held');
listeners.mousemove({ target: button, buttons: 0 }); listeners['doc:mouseleave'](); listeners.mouseover({ target: bar }); listeners['win:blur']();
assert.deepEqual(posted, [true, false, true, false]);
console.log('PASS the native view hears region changes and is cleared when the pointer leaves or the window blurs');
