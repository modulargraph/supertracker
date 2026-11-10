/* Commands that open or drive the feature modules (kit groups, browser, song, clips and takes,
   routing, automation, recording, dialogs, MK3). */
(function () {
  'use strict';

  const app = window.STApp;
  const { state, toast, setView, defineCommands } = app;

  const showView = view => { if (state.view !== view) setView(view); };
  // Clip and take commands always show the Song view first.
  const inSong = run => () => {
    setView('song');
    return run();
  };

  // Song arrangement commands (song-sequence.js) by palette name.
  const SONG_COMMANDS = {
    songPlayFrom: 'playFrom', songQueueFrom: 'queueFrom', songLoop: 'loop', songInsert: 'insert', songInsertNew: 'insertNew',
    songDuplicate: 'duplicate', songClone: 'clone', songRemove: 'remove', songRename: 'rename', songRepeatUp: 'repeatUp',
    songRepeatDown: 'repeatDown', songFollow: 'follow'
  };
  for (const [name, action] of Object.entries(SONG_COMMANDS)) {
    defineCommands({
      [name]: () => {
        showView('song');
        return window.STSong?.command(action);
      }
    });
  }

  // Automation view commands handled by studio.js.
  for (const name of ['automationPatternPrev', 'automationPatternNext', 'automationScope', 'automationCopyLane',
    'automationCopyLaneUsing', 'automationSweep', 'automationMakeUnique']) {
    defineCommands({
      [name]: command => {
        showView('automation');
        return window.STStudio?.command(command);
      }
    });
  }

  function createBus(command) {
    setView('mixer');
    return window.STRouting?.createBus(command === 'addGroup' ? 'group' : 'return').catch(e => toast(e.message, 'error'));
  }

  function browsePlugins() {
    setView('browser');
    return window.STBrowser?.showPlugins?.('instrument');
  }

  const duplicateTrack = () => window.STTrackDuplicate?.duplicate();
  // The cursor's group (a kit) in the mixer: its bus strip, members shown.
  function revealGroup() {
    const group = window.STGroups?.ofTrack(state.project, state.track);
    if (!group) return toast('Move the cursor to a group (kit) to show it in the Mixer.');
    app.setViewPreference(group.id, 'mixerCollapsed', false);
    setView('mixer');
    window.STStudio?.selectOwner(`track:${state.project.tracks[state.track].routingID}`);
    app.refresh();
    window.STStudio?.focusSelectedChannel?.();
    return true;
  }

  defineCommands({
    browseKits: () => window.STBrowser?.showKits?.(),
    browsePlugins,
    sendEffects: () => window.STSends?.show(),
    nextPane: () => window.STPaneNavigation?.next(1),
    previousPane: () => window.STPaneNavigation?.next(-1),
    blockLeft: () => window.STBlockNav?.move('left'),
    blockRight: () => window.STBlockNav?.move('right'),
    blockUp: () => window.STBlockNav?.move('up'),
    blockDown: () => window.STBlockNav?.move('down'),
    focusGrid: () => window.STPaneNavigation?.focusGrid(),

    addKit: () => window.STKitGroups?.createKit(),
    revealSound: revealGroup,
    saveKit: () => window.STKitGroups?.saveKit(),
    saveKitAs: () => window.STKitGroups?.saveKitAs(),
    removeKit: () => window.STKitGroups?.removeKit(),
    duplicateTrack,

    addGroup: createBus,
    addReturn: createBus,
    routing: () => window.STRouting?.show(window.STStudio?.selectedOwner()),

    automationWriteToggle: () => window.STAutomationRecord?.toggleWrite(),
    automationWriteCycle: () => window.STAutomationRecord?.cycleMode(1),
    automationClearArms: () => window.STAutomationRecord?.clearArms(),

    importAudio: inSong(() => window.STClips?.importFile()),
    bounceTrack: inSong(() => window.STClips?.bounceTrack()),
    clipTakes: inSong(() => window.STClipTakes?.show()),
    importTake: inSong(() => window.STClipTakes?.importFile()),
    recordTake: inSong(() => window.STClipTakes?.record()),
    recordInput: () => window.STRecording?.toggleCapture(),
    lineInputs: () => window.STLineInputs?.show(),

    relinkMedia: () => window.STMedia?.open(),
    pluginRecovery: () => window.STPluginRecovery?.show(),
    projectTemplates: () => window.STProjectTemplates?.show(),

    mk3Simulator: command => window.STMK3?.command(command),
    mk3Connect: command => window.STMK3?.command(command),
    mk3Release: command => window.STMK3?.command(command)
  });
})();
