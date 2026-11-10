/* Scalar fields: number/range inputs that edit the project keep their typed draft while
   focused and repaint once the field settles. */
(function () {
  'use strict';

  const app = window.STApp;
  const { state, pattern, currentInstrument, toast, mutate } = app;
  app.scalarFields = null;
  let scalarPointerHeld = false;
  let scalarPointerGuards = false;

  const scalarCheckpoint = () => [state.project, state.view, pattern(), state.track, state.row, state.instrument,
      currentInstrument()];

  const scalarFieldsCurrent = (scope, allowModal = false) => scope === app.scalarFields
      && scope.project === state.project
      && scope.view === state.view
      && (allowModal || !state.modal);

  function paintScalarFields(scope) {
    scope.pending = true;
    clearTimeout(scope.timer);
    scope.timer = setTimeout(() => {
      if (!scalarFieldsCurrent(scope)) return;
      const checkpoint = scalarCheckpoint();
      if (!scope.checkpoint.every((value, index) => value === checkpoint[index])) return;
      if (scalarPointerHeld || scope.drafts.has(document.activeElement)) return;
      scope.pending = false;
      app.refresh();
    }, 0);
  }

  function beginScalarFields() {
    if (app.scalarFields) clearTimeout(app.scalarFields.timer);
    app.scalarFields = {
      project: state.project,
      view: state.view,
      checkpoint: scalarCheckpoint(),
      drafts: new Set(),
      pending: false,
      timer: null
    };
    if (!scalarPointerGuards && document.addEventListener) {
      scalarPointerGuards = true;
      document.addEventListener('pointerdown', () => {
        scalarPointerHeld = true;
      }, true);
      const released = () => {
        scalarPointerHeld = false;
        if (app.scalarFields?.pending) paintScalarFields(app.scalarFields);
      };
      for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, released, true);
      window.addEventListener?.('blur', released);
    }
  }

  // options.allowEmpty: an empty field is a value too (the Gate field: empty is no gate).
  function bindScalarField(input, label, apply, owns = () => true, commit = null, options = {}) {
    const scope = app.scalarFields;
    input.addEventListener('input', () => scope.drafts.add(input));
    // Escape leaves a typed field without committing it, as in the step fields: the stored value comes back
    // before the key moves focus away (which would otherwise commit the draft).
    input.addEventListener('keydown', event => {
      if (event.key !== 'Escape' || input.type === 'range' || !scope.drafts.has(input)) return;
      input.value = input.defaultValue;
      scope.drafts.delete(input);
    });
    input.addEventListener('blur', () => {
      scope.drafts.delete(input);
      if (scope.pending) paintScalarFields(scope);
    });
    input.onchange = event => {
      // Keyboard-opened dialogs may set modal before their focus blurs this field.
      // Accept that owned commit; the paint guard still protects the dialog.
      if (!scalarFieldsCurrent(scope, true) || input.isConnected === false || !owns()) return;
      // A native change can run while Tab/blur still has focus on body. Commit the
      // model now, but keep the adjacent DOM alive until focus or a click lands.
      if (options.allowEmpty && !String(input.value).trim()) {
        mutate(label, p => apply(p, event), { render: false });
      } else if (String(input.value).trim() && Number.isFinite(Number(input.value))) {
        const pending = commit?.(Number(input.value));
        if (pending) {
          scope.drafts.delete(input);
          scope.checkpoint = scalarCheckpoint();
          Promise.resolve(pending).catch(error => toast(error.message, 'error')).finally(() => {
            if (state.project === scope.project && app.scalarFields?.project === scope.project) {
              app.scalarFields.checkpoint = scalarCheckpoint();
              paintScalarFields(app.scalarFields);
            }
          });
          return;
        }
        mutate(label, p => apply(p, event), { render: false });
      } else {
        toast('Enter a finite number for this field.', 'error');
      }
      scope.drafts.delete(input);
      scope.checkpoint = scalarCheckpoint();
      paintScalarFields(scope);
    };
  }

  Object.assign(app, { scalarCheckpoint, paintScalarFields, beginScalarFields, bindScalarField });
})();
