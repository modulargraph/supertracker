/* Window dragging. On macOS the web view fills the whole window (full-size content under a hidden
   title bar), so it receives every click and the system never moves the window. A press on empty top
   bar space is therefore handed to the host (`window.titleBar`), which starts the window drag; a
   double click zooms the window. Hosts whose window has its own title bar, and browsers, do not
   implement the command, and the page then leaves the press alone. */
(() => {
  'use strict';
  const INTERACTIVE = 'button,input,select,textarea,a,label,[contenteditable],[role="button"],[tabindex],[draggable="true"]';

  function draggable(target) {
    if (!(target instanceof Element) || !target.closest('.topbar')) return false;
    if (target.closest(INTERACTIVE)) return false;
    const cursor = getComputedStyle(target).cursor;
    return cursor === 'auto' || cursor === 'default';
  }

  document.addEventListener('mousedown', event => {
    if (event.button !== 0 || !draggable(event.target)) return;
    const st = window.ST;
    if (!st?.hostImplements?.('window.titleBar')) return;
    event.preventDefault();
    st.request('window.titleBar', { doubleClick: event.detail >= 2 }).catch(() => {});
  }, true);

  window.STWindowDrag = { draggable };
})();
