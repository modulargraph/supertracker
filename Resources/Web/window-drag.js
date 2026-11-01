/* Window dragging. The web view fills the whole window (full-size content under a hidden title bar), so
   it receives every click and AppKit never moves the window. While the pointer is over empty top bar
   space, this module tells the native view ('windowDrag' handler); its next mouse-down drags the window
   (double-click zooms or minimises, as the system setting says) instead of reaching the page. */
(() => {
  'use strict';
  const INTERACTIVE = 'button,input,select,textarea,a,label,[contenteditable],[role="button"],[tabindex],[draggable="true"]';
  let sent = false;

  function draggable(target) {
    if (!(target instanceof Element) || !target.closest('.topbar')) return false;
    if (target.closest(INTERACTIVE)) return false;
    const cursor = getComputedStyle(target).cursor;
    return cursor === 'auto' || cursor === 'default';
  }
  function report(value) {
    if (value === sent) return;
    sent = value;
    try { window.webkit?.messageHandlers?.windowDrag?.postMessage(value); } catch { /* not native */ }
  }

  document.addEventListener('mousemove', event => report(event.buttons === 0 ? draggable(event.target) : sent), true);
  document.addEventListener('mouseover', event => report(draggable(event.target)), true);
  document.documentElement.addEventListener('mouseleave', () => report(false));
  window.addEventListener('blur', () => report(false));

  window.STWindowDrag = { draggable };
})();
