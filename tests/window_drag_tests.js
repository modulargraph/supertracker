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
const listeners = {}, requests = [];
const context = {
  Element, getComputedStyle: element => ({ cursor: element.cursor }),
  document: { addEventListener: (type, fn) => (listeners[type] = fn), documentElement: { addEventListener() {} } },
};
// The host says which commands it has; only a host whose window has no title bar of its own has this one.
let hostCommands = new Set(['window.titleBar']);
context.window = { ...context, addEventListener() {}, ST: {
  hostImplements: (command, assumed = true) => hostCommands ? hostCommands.has(command) : assumed,
  request: (command, payload) => { requests.push({ command, payload }); return Promise.resolve(); }
} };
vm.createContext(context);
vm.runInContext(fs.readFileSync(path.join(__dirname, '../Resources/Web/window-drag.js'), 'utf8'), context);

const body = new Element('main'), bar = new Element('header', body, { class: 'topbar' }), brand = new Element('div', bar, { class: 'brand' });
const button = new Element('button', bar), tempo = new Element('input', bar), clickable = new Element('div', bar, {}, 'pointer');
const { draggable } = context.window.STWindowDrag;
assert.equal(draggable(bar), true); assert.equal(draggable(brand), true);
for (const control of [button, tempo, clickable, body]) assert.equal(draggable(control), false);
console.log('PASS empty top bar space drags; controls and the rest of the page do not');

let prevented = 0;
const press = (target, extra = {}) => listeners.mousedown({ target, button: 0, detail: 1, preventDefault: () => prevented++, ...extra });
press(brand); press(bar, { detail: 2 });
assert.deepEqual(requests, [{ command: 'window.titleBar', payload: { doubleClick: false } }, { command: 'window.titleBar', payload: { doubleClick: true } }]);
assert.equal(prevented, 2);
console.log('PASS a press on empty top bar space asks the host to move the window; a double click to zoom it');

press(button); press(tempo); press(body); press(brand, { button: 2 });
assert.equal(requests.length, 2, 'controls, the page and other mouse buttons keep their clicks');
for (const commands of [new Set(['ready']), null]) {
  hostCommands = commands; press(brand);
}
assert.equal(requests.length, 2); assert.equal(prevented, 2);
console.log('PASS a host with its own title bar (or one that does not list its commands) leaves the press to the page');
