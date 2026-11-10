/* Commands: views, session files, dialogs and app-level panels. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    state, mutate, setView, textPrompt, projectAction, midiFileAction, showHelp, openPalette, showSettings, showExport,
    browseNKS, CONTROL_COMMANDS, clickControl, defineCommand, defineCommands, definePrefixCommand
  } = app;

  definePrefixCommand('view:', command => setView(command.slice(5)));

  // Commands that focus or click a control of a view (the catalogue lists view and selector).
  for (const [name, control] of Object.entries(CONTROL_COMMANDS)) defineCommand(name, command => clickControl(command, ...control));

  const renameProject = () => textPrompt('Session name', state.project.name, name => mutate('rename project', p => p.name = name));

  defineCommands({
    new: projectAction,
    open: projectAction,
    save: projectAction,
    saveAs: projectAction,
    demo: () => projectAction('demo'),
    renameProject,
    importMIDI: () => midiFileAction(true),
    exportMIDI: () => midiFileAction(false),
    export: () => showExport(),

    palette: () => openPalette(),
    settings: () => showSettings(),
    help: () => showHelp(),
    browseNKS: command => browseNKS(command === 'browseNKS' ? 'instrument' : 'effect'),
    browseNKSEffects: command => browseNKS(command === 'browseNKS' ? 'instrument' : 'effect')
  });
})();
