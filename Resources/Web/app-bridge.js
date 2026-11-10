/* Host bridge: request() posts commands to the host through host-transport.js and settles them when
   the reply arrives; events from the host go to window.nativeEvent() (app-events.js).
   On a page opened without a host, previewRequest() answers with browser-preview stand-ins. */
(function () {
  'use strict';

  const app = window.STApp;
  const { state, blankProject, toast, download } = app;
  const transport = window.STHostTransport;

  // 'ready' restores the startup session's plug-ins first, which can take longer than any fixed limit; giving up
  // would leave the placeholder project in the window while the host still holds the recovered session.
  const interactiveCommands = new Set(['ready', 'ui.contextMenu', 'freeze.track', 'freeze.unfreeze', 'audio.export',
      'audio.bounceTrack', 'routing.apply', 'project.open', 'project.save', 'project.saveAs', 'project.importMIDI',
      'project.exportMIDI', 'dialog.openSample', 'dialog.export', 'project.media.chooseRoot', 'library.chooseRoot',
      'audio.record', 'audio.inputs', 'audio.inputs.configure', 'audio.configure', 'plugin.load', 'plugin.loadNKS',
      'plugins.recovery.retry', 'kit.read', 'kit.save']);

  const instrumentMutationCommands = new Set(['sample.load', 'sample.edit', 'plugin.load', 'plugin.loadNKS',
      'plugin.parameter', 'plugin.preset', 'plugin.remove', 'plugin.move', 'plugin.duplicate', 'routing.apply',
      'freeze.track', 'freeze.unfreeze', 'audio.export', 'audio.bounceTrack', 'audio.configure']);

  // Commands that change the audio inputs wait for a line-input change like instrument mutations do.
  const lineInputCommands = ['engine.project', 'audio.record', 'audio.inputs', 'audio.inputs.configure'];

  /** The busy operation a native command has to wait for (the caller reports the rejection). */
  function requestBlocker(command, options) {
    const mutation = instrumentMutationCommands.has(command);
    const instrumentProject = command === 'engine.project' && window.STBusy.isBusy('instrumentApply');
    return window.STBusy.blocker('request', [
      !options.lineInputCommit && (mutation || lineInputCommands.includes(command)) && 'lineInputs',
      !options.kitEdit && mutation && 'kitChange',
      !options.instrumentEdit && (mutation || instrumentProject) && 'instrumentEdits'
    ]);
  }

  function request(command, payload = {}, options = {}) {
    const blocked = requestBlocker(command, options);
    if (blocked) return Promise.reject(new Error(blocked.message));
    if (!state.native) return previewRequest(command, payload);
    return new Promise((resolve, reject) => {
      const id = ++state.requestId;
      const timeout = interactiveCommands.has(command) ? null : setTimeout(() => {
        state.pending.delete(id);
        reject(new Error(`${command} took too long. Check the audio device or try again.`));
      }, ['audio.export', 'audio.bounceTrack', 'project.media.search', 'project.media.apply'].includes(command)
          ? 180000
          : ['sample.edit', 'sample.analyzeSlices'].includes(command)
          ? 180000
          : command === 'sample.load' || command === 'plugin.load'
          ? 45000
          : 15000);
      state.pending.set(id, { resolve, reject, timeout, command });
      transport.post({ id, command, payload });
    });
  }

  /** A request with no time limit and no busy checks: for operations that own their busy state and can
      render for minutes (freezing a track, rendering a selection). */
  function longRequest(command, payload = {}) {
    if (!state.native) return previewRequest(command, payload);
    return new Promise((resolve, reject) => {
      const id = ++state.requestId;
      state.pending.set(id, { resolve, reject, timeout: null, command });
      transport.post({ id, command, payload });
    });
  }

  /** Whether the host has a command. `ready` lists the host's commands; for a host that does not say
      (`state.hostCommands` unset), the answer is `assumed`. */
  function hostImplements(command, assumed = true) {
    return state.hostCommands ? state.hostCommands.has(command) : assumed;
  }

  function receiveReply(message) {
    const p = state.pending.get(message.id);
    if (!p) return;
    clearTimeout(p.timeout);
    state.pending.delete(message.id);
    if (message.ok && p.command === 'plugin.state' && message.result?.stateError) {
      p.reject(new Error('Could not capture current plug-in state: ' + message.result.stateError));
    } else if (message.ok) {
      p.resolve(message.result);
    } else {
      p.reject(new Error(message.error || `${p.command} failed`));
    }
  }

  // Replies carry the id of their request; everything else from the host is an event.
  transport?.onMessage(message => {
    if ('id' in message) receiveReply(message);
    else window.nativeEvent?.(message);
  });

  async function previewRequest(command, payload) {
    if (command === 'ready') return {
      project: state.project,
      roots: [],
      plugins: [],
      audio: {},
      midi: {},
      version: 'browser preview'
    };
    if (command === 'engine.project' || command === 'project.autosave') return {};
    if (command === 'transport.play') {
      toast('Interface preview: open the macOS app for audio playback.');
      return {};
    }
    // Seek precedes every Play (which explains the preview); a step audition is silent like a released note.
    if (command === 'transport.stop'
        || command === 'transport.seek'
        || command === 'step.preview'
        || command === 'engine.panic'
        || command === 'note.off'
        || command === 'sample.stopPreview') return {};
    if (command === 'note.on') {
      toast('Computer-keyboard notes play through the native macOS audio engine.');
      return {};
    }
    if (command === 'library.search') return { items: [], total: 0, categories: [], scanning: false };
    if (command === 'library.roots' || command === 'plugins.list') return [];
    if (command === 'audio.devices') return { inputs: [], outputs: [] };
    if (command === 'midi.devices') return { inputs: [], outputs: [] };
    if (command === 'project.save' || command === 'project.saveAs') {
      download(JSON.stringify(state.project, null, 2), state.project.name + '.supertracker', 'application/json');
      state.dirty = false;
      app.updateTransport();
      return { project: state.project };
    }
    if (command === 'project.new') return { project: blankProject() };
    if (command === 'project.open') {
      return new Promise((res, rej) => {
        const f = document.createElement('input');
        f.type = 'file';
        f.accept = '.supertracker,.json';
        f.onchange = async () => {
          try {
            res({ project: JSON.parse(await f.files[0].text()) });
          } catch (e) {
            rej(e);
          }
        };
        f.click();
      });
    }
    throw new Error('This action uses the native macOS app. The browser preview cannot access audio devices, plug-ins, or your sample folders.');
  }

  Object.assign(app, { request, longRequest, hostImplements });
})();
