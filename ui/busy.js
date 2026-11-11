/* Busy registry: the one place that knows which long-running operation is in progress and why
   an action has to wait for it.

   A source is registered once with STBusy.register(id, { active, reason, reasons, onRefuse }):
     active()   true while the operation runs (read live on every check),
     reason     what the user is told by default,
     reasons    wording for particular gates ({ edit: '…', undo: '…' }),
     onRefuse() an optional side effect when it refuses an action (focus a pending dialog).
   The sources below follow the public flags of the feature modules (window.STExport.isBusy, …), so a
   module that is replaced or not loaded is read as it is now; app modules register their internal
   sources (save in progress, session dialog, settings apply) themselves.

   Gates ask with an ordered list of source ids (falsy entries are skipped, so exemptions read as
   `command !== 'recordInput' && 'recording'`):
     STBusy.refuse(gate, ids)  the first busy source, reported to the user (toast) and remembered,
                               or null when the action may run;
     STBusy.blocker(gate, ids) the same without reporting (callers that throw or defer);
     STBusy.any(ids), STBusy.isBusy(id), STBusy.active() for plain questions;
     STBusy.report(gate, message) reports a refusal that is not tied to a source.
   A refusal is never silent: it reaches the reporter (the app's toast) and STBusy.history. */
(function () {
  'use strict';

  const sources = new Map();
  const history = [];
  let reporter = message => window.ST?.toast?.(message);

  function register(id, source) {
    sources.set(id, source);
    return () => { if (sources.get(id) === source) sources.delete(id); };
  }

  function isBusy(id) {
    return !!sources.get(id)?.active();
  }

  function reason(id, gate) {
    const source = sources.get(id);
    return source?.reasons?.[gate] || source?.reason || `Wait for ${id} to finish.`;
  }

  function blocker(gate, ids) {
    for (const id of ids) if (id && isBusy(id)) return { id, gate, message: reason(id, gate) };
    return null;
  }

  function remember(entry) {
    history.push({ ...entry, at: Date.now() });
    if (history.length > 20) history.shift();
  }

  function report(gate, message, id = null) {
    remember({ gate, id, message });
    reporter(message);
  }

  function refuse(gate, ids) {
    const found = blocker(gate, ids);
    if (!found) return null;
    sources.get(found.id).onRefuse?.();
    report(gate, found.message, found.id);
    return found;
  }

  const any = ids => ids.some(id => id && isBusy(id));
  const active = () => [...sources.keys()].filter(isBusy).map(id => ({ id, message: reason(id) }));

  // Sources that follow the feature modules' public flags.
  const flag = (name, property) => () => !!window[name]?.[property];
  const recording = () => !!(window.STRecording?.isRecording || window.STRecording?.isBusy);
  const STANDARD = {
    templates: { active: flag('STProjectTemplates', 'isBusy'), reason: 'Wait for the current template update to finish.' },
    lineInputs: { active: flag('STLineInputs', 'isBusy'), reason: 'Wait for line inputs to finish applying.' },
    export: { active: flag('STExport', 'isBusy'), reason: 'Finish or cancel the audio export first.',
      reasons: {
        perform: 'Finish or cancel the audio export before changing the session.',
        replace: 'Finish or cancel the audio export before changing the session.',
        edit: 'Finish or cancel the audio export before editing the session.',
        undo: 'Finish or cancel the audio export before editing the session.',
        dialog: 'Finish or cancel the audio export before opening another dialog.',
        close: 'Finish or cancel the audio export before closing this window.',
        save: 'Finish or cancel the audio export before saving.',
        keys: 'Finish or cancel the audio export before using session commands.',
        sync: 'Finish or cancel the audio export before updating the audio project.'
      } },
    recording: { active: recording, reason: 'Finish the audio recording first.',
      reasons: {
        perform: 'Finish the audio recording before changing the session.',
        replace: 'Finish the audio recording before changing the session.',
        dialog: 'Finish the audio recording before opening another dialog.',
        close: 'Stop the audio recording before closing this window.',
        palette: 'Finish the audio recording before opening commands.'
      } },
    // Inline recording on the Recording page: an edit would land behind the take being recorded.
    inlineRecording: { active: () => !!window.STRecording?.inlineActive && recording(), reason: 'Stop and load the recording before editing the project.' },
    inlineInput: { active: () => !!window.STRecording?.inlineActive && (recording() || !!window.STRecording.checkingInlineInput),
      reason: 'Stop recording or input checking before opening another dialog.' },
    kits: { active: flag('STKitGroups', 'isBusy'), reason: 'Wait for the kit to finish loading.' },
    instrumentEdits: { active: flag('STInstrumentEdits', 'isBusy'), reason: 'Wait for the instrument or sample change to finish.' },
    instrumentApply: { active: flag('STInstrumentEdits', 'isApplying'), reason: 'Wait for the instrument or sample change to finish.',
      reasons: { sync: 'Wait for the instrument or sample change to finish before updating the audio project.' } },
    routing: { active: flag('STRouting', 'isApplying'), reason: 'Wait for the routing change to finish.',
      reasons: { edit: 'Applying the routing change. Try this edit again in a moment.', undo: 'Applying the routing change. Undo will be available when it finishes.' } },
    routingPrep: { active: flag('STRouting', 'isPreparing'), reason: 'Wait for the routing change to finish.' },
    trackDuplicate: { active: flag('STTrackDuplicate', 'isBusy'), reason: 'Wait for the track duplication to finish.' },
    trackDuplicateApply: { active: flag('STTrackDuplicate', 'isApplying'), reason: 'Wait for the track duplication to finish.' },
    effectMove: { active: flag('STStudio', 'isMovingEffect'), reason: 'Wait for the effect move to finish.' },
    effectMovePrep: { active: flag('STStudio', 'isPreparingEffectMove'), reason: 'Wait for the effect move to finish.' },
    pluginEdit: { active: flag('STStudio', 'isEditingPlugin'), reason: 'Wait for the plug-in edit to finish.' },
    automationWrite: { active: flag('STAutomationRecord', 'isRecording'), reason: 'Finish the automation pass first.' },
    // A save or a session change (window.ST.isChangingSession); app-session.js registers its parts.
    session: { active: flag('ST', 'isChangingSession'), reason: 'Wait for the current save or project change to finish.' }
  };
  for (const [id, source] of Object.entries(STANDARD)) register(id, source);

  window.STBusy = {
    register, isBusy, reason, blocker, refuse, report, any, active, history,
    setReporter(fn) { reporter = fn; }
  };
})();
