/* Native event interceptors: a module that owns an in-flight native operation (a kit import, an
   instrument edit, a track duplication) can claim the plug-in and sample events that belong to it
   before the app applies them to the open project.

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

  // Plug-in list events carry the list either bare or as payload.plugins.
  const PLUGIN_EVENTS = ['plugins.changed', 'audioPlugins.changed'];
  const plugins = payload => payload?.plugins || payload;

  window.STNativeEvents = { intercept, consume, PLUGIN_EVENTS, plugins };
})();
