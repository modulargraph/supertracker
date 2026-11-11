/* Host event interceptors: a module that owns an operation in flight (an instrument edit, a render)
   or a kind of event of its own (AI control) can claim events before the app handles them.

     STNativeEvents.intercept(types, handler, order)  register; handler(payload, type) returns true
                                                       to consume the event. Lower order runs first.
     STNativeEvents.consume(type, payload)            ask the interceptors (window.nativeEvent does);
                                                       true when one of them consumed the event. */
(function () {
  'use strict';

  const interceptors = [];

  function intercept(types, handler, order = 100) {
    const entry = { types: new Set([].concat(types)), handler, order };
    interceptors.push(entry);
    interceptors.sort((a, b) => a.order - b.order);
    return () => {
      const index = interceptors.indexOf(entry);
      if (index >= 0) interceptors.splice(index, 1);
    };
  }

  function consume(type, payload) {
    for (const { types, handler } of [...interceptors]) if (types.has(type) && handler(payload, type)) return true;
    return false;
  }

  window.STNativeEvents = { intercept, consume };
})();
