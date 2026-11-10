# Project templates

Templates are reusable starting projects stored separately from the current document. Saving or selecting a default template does not replace the open session, change its save location or dirty state, or overwrite its recovery copy. Template media is collected into the template folder. Plug-in state is preserved as project data; saving template data does not instantiate plug-ins or send MIDI.

New uses the configured default, or the ordinary blank project if no default is selected. An explicit template can be used without changing the default. Demo and explicit blank creation bypass it. Startup restores Recovery first and uses the default only when Recovery is absent. Before anything can rewrite it, each launch keeps a copy of the Recovery file it found in `Recovery Backups` next to it (the last five distinct sessions), so an unsaved session that a relaunch replaced can still be opened with File > Open. An unreadable startup project produces a visible warning and a blank session; an unreadable template requested from an existing session fails before replacing that session.

Native bridge commands:

- `project.templates.list` returns `templates` (id, name, path) and `defaultID`.
- `project.templates.save` accepts `name`, a complete `project`, and optional `id` for updating a template. It returns template metadata. Callers should capture pending native editor state first.
- `project.templates.setDefault` accepts `id`, or `null` to restore ordinary blank defaults. The template must be readable before it becomes the default.
- `project.new` accepts optional `templateID`, or `blank: true` to bypass defaults. It returns `{project: ...}` for a template and null for the ordinary web blank factory.

Files live under the application's support directory: `Templates/<id>/Template.supertracker`, collected `Template.media`, `Metadata.json`, and `ProjectTemplates.json` for the default preference. `SUPERTRACKER_SUPPORT_DIR` isolates this storage for tests.
