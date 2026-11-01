/* Command registry: every named command (menus, palette, keyboard, MK3, agent) maps to one small
   handler. Handlers are defined in the app-commands-*.js tables; the palette metadata (label,
   shortcut, aliases) lives in the command catalogue (app-command-catalog.js).

   defineCommands({ name: handler }) registers exact names; definePrefixCommand('view:', handler)
   registers a family. A handler receives the command name. perform() first asks the busy gate,
   then runs the handler; unknown commands do nothing. */
(function () {
  'use strict';

  const app = window.STApp;
  const { commands } = app;
  const busy = window.STBusy;

  const handlers = new Map(), prefixes = [];

  function defineCommand(names, handler) {
    for (const name of [].concat(names)) {
      if (handlers.has(name)) throw new Error(`Command ${name} is defined twice`);
      handlers.set(name, handler);
    }
  }

  function defineCommands(table) {
    for (const [name, handler] of Object.entries(table)) defineCommand(name, handler);
  }

  function definePrefixCommand(prefix, handler) {
    prefixes.push([prefix, handler]);
  }

  function commandHandler(command) {
    if (typeof command !== 'string') return null;
    return handlers.get(command) || prefixes.find(([prefix]) => command.startsWith(prefix))?.[1] || null;
  }

  const hasCommand = command => !!commandHandler(command);
  /** Palette metadata for a command: [id, label, shortcut, aliases]. */
  const commandInfo = command => commands.find(entry => entry[0] === command) || null;

  function perform(command) {
    // Stop always runs; everything else waits for operations that own the session.
    if (command !== 'stop') {
      if (busy.refuse('perform', ['basePatch', 'lineInputs', 'settings'])) return false;
      if (busy.refuse('perform', ['export', command !== 'recordInput' && 'recording'])) return;
    }
    const handler = commandHandler(command);
    return handler ? handler(command) : undefined;
  }

  Object.assign(app, { defineCommand, defineCommands, definePrefixCommand, commandHandler, hasCommand, commandInfo, perform });
})();
