/* Shortcut labels for the platform the page runs on. The interface writes shortcuts with the Mac's
   symbols (⌘ ⌥ ⇧ ⌃) and names (Cmd, Option). The keys themselves already work everywhere: the
   keyboard handler treats Ctrl like Cmd. Away from a Mac this module shows them as Ctrl / Alt / Shift:
   it rewrites text and tooltips as they enter the page, so no template has to know the platform. */
(function (root) {
  'use strict';

  const isMac = platform => /mac|iphone|ipad/i.test(String(platform || ''));
  const SYMBOLS = { '⌘': 'Ctrl', '⌃': 'Ctrl', '⌥': 'Alt', '⇧': 'Shift' };
  const NAMES = { Cmd: 'Ctrl', Command: 'Ctrl', Option: 'Alt', Opt: 'Alt' };
  const SYMBOL_RUN = /[⌘⌃⌥⇧](?:\s?[⌘⌃⌥⇧])*\s?(?:\+\s?)?/g, NAMED = /\b(Cmd|Command|Option|Opt)(?=\s?[+-]\s?\S)/g;

  // "⌘⇧Z" -> "Ctrl+Shift+Z", "⌥ ↑" -> "Alt+↑", "Cmd+K" -> "Ctrl+K". Text without shortcuts is returned as is.
  function translate(text) {
    if (typeof text !== 'string' || !/[⌘⌃⌥⇧]|Cmd|Command|Opt/.test(text)) return text;
    return text
      .replace(SYMBOL_RUN, run => {
        const names = [];
        for (const symbol of run) if (SYMBOLS[symbol] && !names.includes(SYMBOLS[symbol])) names.push(SYMBOLS[symbol]);
        return names.join('+') + '+';
      })
      .replace(NAMED, name => NAMES[name]);
  }

  const ATTRIBUTES = ['title', 'aria-label', 'aria-keyshortcuts', 'placeholder', 'data-shortcut'];
  function translateNode(node) {
    if (node.nodeType === 3) {
      const next = translate(node.nodeValue);
      if (next !== node.nodeValue) node.nodeValue = next;
      return;
    }
    if (node.nodeType !== 1) return;
    for (const name of ATTRIBUTES) {
      const value = node.getAttribute(name);
      if (value) { const next = translate(value); if (next !== value) node.setAttribute(name, next); }
    }
  }
  function translateTree(rootNode) {
    translateNode(rootNode);
    if (rootNode.nodeType !== 1) return;
    const walker = rootNode.ownerDocument.createTreeWalker(rootNode, 5 /* elements and text */);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) translateNode(node);
  }

  function install(document, platform) {
    if (isMac(platform) || typeof root.MutationObserver !== 'function') return false;
    const observer = new root.MutationObserver(records => {
      for (const record of records) {
        if (record.type === 'childList') record.addedNodes.forEach(translateTree);
        else translateNode(record.target);
      }
    });
    observer.observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ATTRIBUTES });
    translateTree(document.documentElement);
    return true;
  }

  const api = { translate, isMac, install };
  if (typeof module === 'object' && module.exports) module.exports = api;
  if (root && root.document && root.navigator) {
    root.STPlatformKeys = api;
    install(root.document, root.navigator.userAgentData?.platform || root.navigator.platform);
  }
})(typeof window === 'object' ? window : null);
