import Foundation

/// `SuperTracker --mcp`: a stdio Model Context Protocol server that proxies tool calls to the
/// running app over the local control socket (AgentControlProtocol.swift). It opens no window and
/// starts no audio. `SuperTracker --ctl <method> '<json>'` calls one control method for scripting.
struct MCPTool {
    let name: String
    let method: String
    let description: String
    let properties: [String: Any]
    let required: [String]

    var definition: [String: Any] {
        var schema: [String: Any] = ["type": "object", "properties": properties, "additionalProperties": false]
        if !required.isEmpty { schema["required"] = required }
        return ["name": name, "description": description, "inputSchema": schema]
    }
}

enum MCPSchema {
    static func string(_ description: String, _ values: [String]? = nil) -> [String: Any] {
        var schema: [String: Any] = ["type": "string", "description": description]
        if let values { schema["enum"] = values }
        return schema
    }
    static func integer(_ description: String, minimum: Int? = nil, maximum: Int? = nil) -> [String: Any] {
        var schema: [String: Any] = ["type": "integer", "description": description]
        if let minimum { schema["minimum"] = minimum }
        if let maximum { schema["maximum"] = maximum }
        return schema
    }
    static func number(_ description: String) -> [String: Any] { ["type": "number", "description": description] }
    static func boolean(_ description: String) -> [String: Any] { ["type": "boolean", "description": description] }
    static func ref(_ description: String) -> [String: Any] { ["type": ["integer", "string"], "description": description] }
    static func value(_ description: String) -> [String: Any] { ["type": ["number", "string"], "description": description] }
    static func object(_ description: String, _ properties: [String: Any] = [:], required: [String] = [], additional: Bool = true) -> [String: Any] {
        var schema: [String: Any] = ["type": "object", "description": description]
        if !properties.isEmpty { schema["properties"] = properties }
        if !required.isEmpty { schema["required"] = required }
        if !additional { schema["additionalProperties"] = false }
        return schema
    }
    static func array(_ description: String, _ items: [String: Any]) -> [String: Any] { ["type": "array", "description": description, "items": items] }

    static let track = ref("Track: 0-based index (the UI shows 01 for track 0) or track name.")
    static let pattern = ref("Pattern id (see get_song) or name. Default: the pattern shown in the editor.")
    static let instrument = ref("Instrument index or name.")
    static let channel = ref("Mixer channel: \"master\", a bus name or \"bus:<id>\", or a track index/name (see get_mixer).")
    static let channels: [String: Any] = ["type": ["array", "integer", "string"], "items": ["type": ["integer", "string"]], "description": "Mixer channel or list of channels: \"master\", a bus name or \"bus:<id>\", or a track index/name."]
    static let label = string("Optional undo label (shown as \"AI: <label>\" in Edit › Undo).")
    static let from = integer("First row (0-based, inclusive). Default 0.", minimum: 0)
    static let to = integer("Last row (inclusive). Default: last row.", minimum: 0)
    static let tracks: [String: Any] = ["type": ["array", "integer", "string"], "items": ["type": ["integer", "string"]], "description": "Track or list of tracks (index or name). Default: all tracks."]
    static let fx = ["type": ["string", "object", "null"], "description": "Step effect, e.g. \"probability 50%\", \"ratchet 3\", \"pitch -12\", \"cutoff 800Hz\", \"pan -30%\", \"reverse\", \"arpeggio 37\" (+3 +7), \"microtiming 25%\", \"volume 80%\", \"slice 4\", \"glide -12\", \"delay 40%\", \"reverb 30%\", \"offset 25%\", \"resonance 50%\". null removes it."] as [String: Any]
}

enum MCPCatalogue {
    typealias S = MCPSchema
    static let tools: [MCPTool] = [
        MCPTool(name: "get_song", method: "summary", description: "Overview of the song open in SuperTracker: name, tempo, meter, rows per bar, tracks (instrument, mute/solo, volume, pan, sends, effects), instruments (type, sample, plug-in, NKS), patterns (id, name, length, note count), the song sequence as blocks, send effects, keyboard performance (scale/chords) and undo history. Call this first.", properties: [:], required: []),
        MCPTool(name: "get_ui_state", method: "uiState", description: "What the user currently sees: view, open dialog, pattern, cursor track/row/column and the step under it, selection, current instrument, transport position.", properties: [:], required: []),
        MCPTool(name: "screenshot", method: "app.screenshot", description: "Capture the SuperTracker window as a PNG image to see exactly what the user sees.", properties: ["width": S.integer("Image width in pixels (320–2560). Default: the window width.", minimum: 320, maximum: 2560)], required: []),
        MCPTool(name: "read_pattern", method: "readPattern", description: "Read a pattern's steps: per track, each non-empty row with note name (C-4 = MIDI 60), instrument, velocity (1–127), gate (rows; only when the note has one), chord intervals and FX in readable form, plus automation lanes. format \"grid\" returns tracker-style text lines (compact for whole patterns).", properties: ["pattern": S.pattern, "tracks": S.tracks, "from": S.from, "to": S.to, "format": S.string("\"steps\" (default, JSON) or \"grid\" (text lines).", ["steps", "grid"]), "includeEmpty": S.boolean("Also list empty rows (steps format).")], required: []),
        MCPTool(name: "write_steps", method: "writeSteps", description: "Write notes/steps into a pattern as ONE undo step. Each step: {track, row, note} with optional instrument, velocity, gate, chord, fx1, fx2; or notes:[...] for a chord; or clear:true. Existing steps are merged (only given fields change). Notes: \"C-4\"/\"C4\"/\"C#4\"/\"Db3\", MIDI numbers, \"OFF\" (note off: the track's notes release), \"CUT\" (they stop at once); note:null keeps only the FX. New steps use the track's instrument, velocity 100 and no gate: like a classic tracker a note plays until the next note on its track, an OFF or a CUT (one-shot samples end by themselves). Give gate only for a fixed length. column (0–7) picks a note column: a track plays up to 8 notes per row, each column its own voice (a new note, OFF or CUT ends only its own column); columns 1–7 hold note, instrument, velocity and gate, and the row's fx1/fx2 (column 0) apply to them.", properties: [
            "pattern": S.pattern, "label": S.label,
            "steps": S.array("Steps to write (up to 4096).", S.object("One step.", [
                "track": S.track, "row": S.integer("Row (0-based).", minimum: 0),
                "column": S.integer("Note column, 0-based (default 0). Writing a column adds it to the track.", minimum: 0, maximum: 7),
                "note": ["type": ["string", "integer", "null"], "description": "Note name, MIDI number, OFF, CUT, or null for an FX-only step."],
                "notes": S.array("Chord notes; the first is the root (sets note and chord).", ["type": ["string", "integer"]]),
                "instrument": S.instrument, "velocity": S.integer("1–127.", minimum: 1, maximum: 127),
                "gate": ["type": ["number", "null"], "description": "Optional note length in rows (0.01–256; 0.8 ≈ most of a row). Omit for none (plays until the next note, OFF or CUT); 0 or null removes a gate."],
                "chord": ["type": ["array", "string", "null"], "items": ["type": "integer"], "description": "Chord as semitone intervals [0,4,7] or a chord name (major, minor, sus2, sus4, ...). null removes it."],
                "fx1": S.fx, "fx2": S.fx, "clear": S.boolean("true clears the step (with column: that note column; column 0 keeps the other columns).")
            ], required: ["track", "row"]))
        ], required: ["steps"]),
        MCPTool(name: "clear_steps", method: "clearSteps", description: "Clear a range of rows on some or all tracks, every note column included (one undo step).", properties: ["pattern": S.pattern, "tracks": S.tracks, "from": S.from, "to": S.to, "label": S.label], required: []),
        MCPTool(name: "insert_rows", method: "insertRows", description: "Insert empty rows at a row; content below moves down and rows past the pattern end drop off.", properties: ["pattern": S.pattern, "row": S.integer("Row to insert at.", minimum: 0), "count": S.integer("Rows to insert (default 1).", minimum: 1), "tracks": S.tracks, "label": S.label], required: ["row"]),
        MCPTool(name: "delete_rows", method: "deleteRows", description: "Delete rows at a row; content below moves up and empty rows fill the end.", properties: ["pattern": S.pattern, "row": S.integer("First row to delete.", minimum: 0), "count": S.integer("Rows to delete (default 1).", minimum: 1), "tracks": S.tracks, "label": S.label], required: ["row"]),
        MCPTool(name: "transform_steps", method: "transform", description: "Transform notes in a range as one undo step. operation: transpose (amount semitones), humanize (amount = max velocity change, default 12), velocity (scale e.g. 1.2, or set 1–127), quantize (removes microtiming FX), snapToScale (scale + root), reverse, rotate (amount rows), gate (set rows; 0 removes gates so notes play until the next note), instrument (set), removeFX.", properties: ["pattern": S.pattern, "tracks": S.tracks, "from": S.from, "to": S.to, "operation": S.string("Operation.", ["transpose", "humanize", "velocity", "quantize", "snapToScale", "reverse", "rotate", "gate", "instrument", "removeFX"]), "amount": S.number("transpose/rotate/humanize amount."), "scale": ["type": ["number", "string"], "description": "velocity: factor; snapToScale: scale name (major, minor, dorian, pentatonic, blues, ...)."], "root": S.ref("snapToScale root, e.g. \"C\", \"F#\" or 0–11."), "set": ["type": ["number", "string", "integer"], "description": "velocity/gate value or instrument (index/name)."], "label": S.label], required: ["operation"]),
        MCPTool(name: "create_pattern", method: "createPattern", description: "Create an empty pattern (selected in the editor unless select:false).", properties: ["name": S.string("Pattern name."), "length": S.integer("Rows (1–256). Default: the current pattern's length.", minimum: 1, maximum: 256), "appendToSong": S.boolean("Also append it to the song sequence."), "select": S.boolean("Show it in the editor (default true)."), "label": S.label], required: []),
        MCPTool(name: "duplicate_pattern", method: "duplicatePattern", description: "Copy a pattern (steps and automation) to a new pattern.", properties: ["pattern": S.pattern, "name": S.string("Name of the copy."), "appendToSong": S.boolean("Append the copy to the song."), "select": S.boolean("Show the copy (default true)."), "label": S.label], required: []),
        MCPTool(name: "rename_pattern", method: "renamePattern", description: "Rename a pattern.", properties: ["pattern": S.pattern, "name": S.string("New name.")], required: ["name"]),
        MCPTool(name: "delete_pattern", method: "deletePattern", description: "Delete a pattern and remove it from the song sequence.", properties: ["pattern": S.pattern, "label": S.label], required: ["pattern"]),
        MCPTool(name: "set_pattern_length", method: "setPatternLength", description: "Set a pattern's length in rows (1–256). Shortening drops steps and automation past the end.", properties: ["pattern": S.pattern, "length": S.integer("Rows.", minimum: 1, maximum: 256), "label": S.label], required: ["length"]),
        MCPTool(name: "edit_song_sequence", method: "editSong", description: "Edit the song arrangement. The song is a list of blocks (one pattern repeated N times). Actions: set {sequence:[pattern | {pattern, repeats}]}, insert {pattern, block?, before?, repeats?} (no block = append), remove {block, toBlock?}, repeats {block, repeats}, replace {block, pattern}, move {block, toBlock?, delta -1|1}, duplicate {block, toBlock?}, loop {block, toBlock?} (loop that section), clearLoop.", properties: ["action": S.string("Action.", ["set", "insert", "remove", "repeats", "replace", "move", "duplicate", "loop", "clearLoop"]), "sequence": S.array("For set.", ["type": ["integer", "string", "object"]]), "pattern": S.pattern, "block": S.integer("Block index (see get_song song.blocks).", minimum: 0), "toBlock": S.integer("Last block of a range.", minimum: 0), "before": S.boolean("Insert before the block instead of after."), "repeats": S.integer("Repeats 1–64.", minimum: 1, maximum: 64), "delta": S.integer("move direction: -1 earlier, 1 later.", minimum: -1, maximum: 1), "label": S.label], required: ["action"]),
        MCPTool(name: "add_track", method: "addTrack", description: "Add a track (no fixed limit; 1024 is a sanity bound).", properties: ["name": S.string("Track name."), "instrument": S.instrument, "label": S.label], required: []),
        MCPTool(name: "edit_track", method: "editTrack", description: "Change a track: name, mute, solo, volume (\"-6 dB\" or 0–1.5 gain), pan (\"30% L\", \"center\", or -1…1), delaySend/reverbSend (\"40%\" or 0–1), instrument.", properties: ["track": S.track, "name": S.string("New name."), "mute": S.boolean("Mute."), "solo": S.boolean("Solo."), "volume": S.value("Volume."), "pan": S.value("Pan."), "delaySend": S.value("Delay send."), "reverbSend": S.value("Reverb send."), "instrument": S.instrument, "label": S.label], required: ["track"]),
        MCPTool(name: "set_song_settings", method: "setSong", description: "Song settings: name, bpm (20–400), swing (\"10%\" or 0–0.75), timeSignature (\"3/4\"), linesPerBeat (rows per beat: 1,2,3,4,6,8,12,16), masterVolume (\"-3 dB\"), loopSong, metronome, metronomeVolume.", properties: ["name": S.string("Song name."), "bpm": S.value("Tempo in BPM."), "swing": S.value("Swing."), "timeSignature": ["type": ["string", "object"], "description": "e.g. \"4/4\" or {numerator, denominator}."], "linesPerBeat": S.integer("Rows per beat.", minimum: 1, maximum: 16), "masterVolume": S.value("Master volume."), "loopSong": S.boolean("Loop song playback."), "metronome": S.boolean("Metronome on/off."), "metronomeVolume": S.value("Metronome level."), "label": S.label], required: []),
        MCPTool(name: "get_instrument", method: "getInstrument", description: "Read an instrument with every parameter in readable units, its range and options.", properties: ["instrument": S.instrument], required: []),
        MCPTool(name: "create_instrument", method: "createInstrument", description: "Create a sampler or external MIDI instrument, optionally setting parameters and assigning it to a track (one undo step). For a plain tone use the sampler's Wavetable mode with a built-in table (load_sample with a builtin:wavetables/... path). For plug-ins use load_plugin / load_nks.", properties: ["type": S.string("Instrument type.", ["sample", "midi"]), "name": S.string("Name."), "params": S.object("Parameters by name with units, e.g. {\"mode\":\"wavetable\",\"wavetablePosition\":\"25%\",\"cutoff\":\"800 Hz\",\"attack\":\"5 ms\",\"volume\":\"-6 dB\"}."), "track": S.track, "label": S.label], required: []),
        MCPTool(name: "edit_instrument", method: "editInstrument", description: "Rename an instrument or set parameters by name with units. Parameters: volume, pan, tune (semitones), tuneSemi, tuneFine (cents), attack, decay, sustain, release, cutoff, resonance, filterType, drive, bitDepth, filter envelope (filterAttack…filterEnvAmount), LFOs (lfoRate, lfoDepth, lfoTarget, lfoShape, lfoSync, lfoBeats, lfo2…), sampler (rootNote, start, end, loopStart, loopEnd, mode, sliceCount, grainSize, grainPosition, wavetablePosition, wavetableFrameSize).", properties: ["instrument": S.instrument, "name": S.string("New name."), "type": S.string("Switch type.", ["sample", "midi"]), "params": S.object("Parameter values, e.g. {\"cutoff\":\"1.2 kHz\",\"resonance\":\"40%\"}."), "label": S.label], required: []),
        MCPTool(name: "search_library", method: "searchLibrary", description: "Search the user's sample library (and NKS presets with kind \"nks\"/\"all\"). Returns paths usable with load_sample.", properties: ["query": S.string("Search words, e.g. \"808 kick\"."), "kind": S.string("Kind.", ["sample", "nks", "all"]), "limit": S.integer("Max results (default 20).", minimum: 1, maximum: 200), "offset": S.integer("Skip results.", minimum: 0), "favorites": S.boolean("Favorites only.")], required: []),
        MCPTool(name: "load_sample", method: "loadSample", description: "Load an audio file (path) or the best library match (query) into an instrument, making it a sampler, and assign it to a track. Default instrument: the track's.", properties: ["path": S.string("Absolute path to an audio file."), "query": S.string("Library search words instead of a path."), "instrument": S.instrument, "track": S.track], required: []),
        MCPTool(name: "list_plugins", method: "listPlugins", description: "List installed plug-ins (AU, VST3, VST2). One entry per plug-in (AU preferred) unless allFormats.", properties: ["query": S.string("Filter words (name/manufacturer)."), "type": S.string("Plug-in type.", ["instrument", "effect"]), "format": S.string("AU, VST3 or VST2."), "limit": S.integer("Max results (default 50).", minimum: 1, maximum: 500), "allFormats": S.boolean("List every format separately."), "refresh": S.boolean("Re-read the plug-in list from the host.")], required: []),
        MCPTool(name: "load_plugin", method: "loadPlugin", description: "Load a plug-in on a track: kind \"instrument\" (plays the track's notes) or \"effect\" (insert slot 0–15). channel instead of track loads an insert effect on a bus or the master channel (\"master\", bus name or \"bus:<id>\"; standard routing). Plug-in names (built-in effects such as \"Reverb\" or \"Peak Limiter\" too) are matched like list_plugins; use the id when ambiguous.", properties: ["plugin": S.string("Plug-in name or id."), "track": S.track, "channel": S.channel, "kind": S.string("Kind.", ["instrument", "effect"]), "slot": S.integer("Effect slot 0–15 (default: next free).", minimum: 0, maximum: 15), "instrument": S.ref("For instruments: instrument index to replace, or \"new\". Default: the track's own instrument if no other track uses it.")], required: ["plugin"]),
        MCPTool(name: "get_plugin_parameters", method: "pluginParameters", description: "List a plug-in's parameters (name, address, current value and display, range) for a track's instrument or an effect slot of a track, bus or master (channel).", properties: ["track": S.track, "channel": S.channel, "kind": S.string("Kind.", ["instrument", "effect"]), "slot": S.integer("Effect slot.", minimum: 0, maximum: 15), "query": S.string("Filter words."), "limit": S.integer("Max results (default 100).", minimum: 1), "offset": S.integer("Skip results.", minimum: 0)], required: []),
        MCPTool(name: "set_plugin_parameters", method: "setPluginParameters", description: "Set plug-in parameters by name or address as one undo step, on a track's instrument or an effect slot of a track, bus or master (channel). Values: \"50%\" or {\"normalized\":0.5} = knob position; a plain number = the parameter's own value within min…max.", properties: ["track": S.track, "channel": S.channel, "kind": S.string("Kind.", ["instrument", "effect"]), "slot": S.integer("Effect slot.", minimum: 0, maximum: 15), "values": S.object("e.g. {\"Cutoff\":\"35%\",\"Resonance\":0.4}."), "label": S.label], required: ["values"]),
        MCPTool(name: "search_nks", method: "searchNKS", description: "Search the NKS preset catalogue (Komplete/Kontakt/Massive and other NKS presets).", properties: ["query": S.string("Search words, e.g. \"warm pad\"."), "kind": S.string("Preset kind.", ["instrument", "effect"]), "product": S.string("Limit to a product, e.g. \"Massive X\"."), "limit": S.integer("Max results (default 20).", minimum: 1, maximum: 200), "offset": S.integer("Skip results.", minimum: 0)], required: []),
        MCPTool(name: "load_nks", method: "loadNKS", description: "Load an NKS preset (path from search_nks, or the best match for query) on a track as its instrument, or as an effect in a slot.", properties: ["path": S.string("Absolute .nksf/.nksfx path."), "query": S.string("Search words instead of a path."), "track": S.track, "kind": S.string("Kind.", ["instrument", "effect"]), "slot": S.integer("Effect slot 0–15; defaults to the first free slot (a full rack needs an explicit slot to replace).", minimum: 0, maximum: 15), "instrument": S.ref("\"new\" to add a new instrument instead of replacing the track's.")], required: []),
        MCPTool(name: "list_automation_targets", method: "automationTargets", description: "List parameters that can be automated on a track, bus or master (channel): mixer, instrument, sends, plug-in parameters, with ranges and current values.", properties: ["track": S.track, "channel": S.channel, "query": S.string("Filter words.")], required: []),
        MCPTool(name: "set_automation", method: "setAutomation", description: "Create or replace an automation lane for a track, bus or master (channel) parameter in a pattern. points [{row, value}] use the parameter's units (\"800 Hz\", \"-6 dB\", or numbers in its range; plug-in targets 0–1 or \"50%\"). Buses: volume, pan, send:<id>, plug-in effects; master: volume and plug-in effects (standard routing). merge:true keeps existing points at other rows.", properties: ["pattern": S.pattern, "track": S.track, "channel": S.channel, "target": S.string("Target id from list_automation_targets, e.g. \"cutoff\", \"volume\", \"pan\", \"instrumentVolume\", \"send:<id>\", \"plugin:instrument:0:<address>\", \"plugin:effect:<slot>:<address>\"."), "points": S.array("Breakpoints.", S.object("Point.", ["row": S.integer("Row.", minimum: 0), "value": S.value("Value.")], required: ["row", "value"])), "interpolation": S.string("Curve between points.", ["linear", "step"]), "merge": S.boolean("Merge with existing points."), "label": S.label], required: ["target", "points"]),
        MCPTool(name: "delete_automation", method: "deleteAutomation", description: "Delete a track's, bus's or master's (channel) automation lane for a target in a pattern.", properties: ["pattern": S.pattern, "track": S.track, "channel": S.channel, "target": S.string("Target id."), "label": S.label], required: ["target"]),
        MCPTool(name: "get_mixer", method: "mixer", description: "The mixer: mix model (legacy/standard), master (volume, inserts), group/return buses (id, level, pan, mute/solo, output, inputs, sends, inserts with slot/format/bypass), every track's level, output, sends and inserts, sidechains, and the built-in delay/reverb send effects. Use it to find bus ids and insert slots. channel limits it to one channel.", properties: ["channel": S.channel], required: []),
        MCPTool(name: "edit_channel", method: "editChannel", description: "Change mixer channels as one undo step: name, volume (\"-6 dB\"; tracks and master up to 1.5 gain, buses up to 2), pan (\"30% L\"), mute, solo, output (a group/return bus, \"master\" or \"none\"; standard routing). channel may list several channels (not for name). Master takes volume only.", properties: ["channel": S.channels, "name": S.string("New name (one channel)."), "volume": S.value("Volume."), "pan": S.value("Pan."), "mute": S.boolean("Mute."), "solo": S.boolean("Solo."), "output": ["type": ["integer", "string", "null"], "description": "Output destination: a bus name or \"bus:<id>\", \"master\", or \"none\"/null to disconnect."], "label": S.label], required: ["channel"]),
        MCPTool(name: "create_bus", method: "createBus", description: "Create a group or return bus (standard routing) as one undo step. inputs: tracks/buses whose output moves to the new bus (group buses); return buses are fed with set_send. The bus outputs to Master unless output says otherwise.", properties: ["name": S.string("Bus name."), "type": S.string("Bus type (default group).", ["group", "return"]), "inputs": S.channels, "output": S.channel, "volume": S.value("Volume, e.g. \"-3 dB\" (0–2 gain)."), "pan": S.value("Pan."), "label": S.label], required: []),
        MCPTool(name: "remove_bus", method: "removeBus", description: "Remove a group or return bus with its inserts, sends and automation (one undo step). Channels that output to it move to the bus's own output. A track group's (kit's) bus is removed by dissolving the group instead.", properties: ["channel": S.channel, "label": S.label], required: ["channel"]),
        MCPTool(name: "set_send", method: "setSend", description: "Add, change or remove a custom send (standard routing) from a track or bus to a group/return bus or Master, as one undo step. A send from→to that exists is edited; otherwise it is created (gain 0 dB, post-fader). The built-in delay/reverb sends are edit_track delaySend/reverbSend.", properties: ["from": S.channel, "to": S.channel, "id": S.string("Send id (see get_mixer), instead of from/to."), "gain": S.value("Send gain, e.g. \"-12 dB\" or 0–2."), "tap": S.string("Tap.", ["postFader", "preFader"]), "enabled": S.boolean("On/off."), "remove": S.boolean("true removes the send and its automation."), "label": S.label], required: []),
        MCPTool(name: "set_send_effects", method: "setSendEffects", description: "Settings of the built-in delay and reverb send effects (one undo step). delay: time (\"375 ms\" or a tempo division like \"1/8.\", \"1/16\", \"1/4T\"), feedback (0–95%), tone (100% = open), mix (return level). reverb: size, damping, predelay (0–250 ms), mix.", properties: ["delay": S.object("Delay settings, e.g. {\"time\":\"1/8.\",\"feedback\":\"40%\",\"mix\":\"30%\"}.", ["time": S.value("Time."), "feedback": S.value("Feedback."), "tone": S.value("Tone."), "mix": S.value("Return level.")], additional: false), "reverb": S.object("Reverb settings, e.g. {\"size\":\"80%\",\"predelay\":\"20 ms\"}.", ["size": S.value("Size."), "damping": S.value("Damping."), "predelay": S.value("Pre-delay."), "mix": S.value("Return level.")], additional: false), "label": S.label], required: []),
        MCPTool(name: "edit_insert", method: "editInsert", description: "Bypass, remove or reorder an insert effect of a track, bus or master (channel) by slot, as one undo step: exactly one of bypass (true/false), remove (true; its automation goes too) or moveTo (another insert's slot, \"first\" or \"last\"; the others shift). Add inserts with load_plugin.", properties: ["channel": S.channel, "track": S.track, "slot": S.integer("Insert slot (see get_mixer).", minimum: 0, maximum: 15), "bypass": S.boolean("Bypass on/off."), "remove": S.boolean("true removes the insert."), "moveTo": ["type": ["integer", "string"], "description": "Slot of the insert whose position it takes, \"first\" or \"last\"."], "label": S.label], required: ["slot"]),
        MCPTool(name: "enable_standard_routing", method: "enableStandardRouting", description: "Switch a legacy-mix song to standard routing so it can have buses, track outputs, custom sends and master inserts (one undo step). It changes the gain order (track faders after inserts, master gain after the routing graph): ask the user first.", properties: ["label": S.label], required: []),
        MCPTool(name: "set_performance", method: "setPerformance", description: "Keyboard performance settings used when the user plays the computer keyboard / MIDI: scale, root, chord, chordSet (\"\" = off), scaleMode (snap/degrees), octave. Call with no arguments to list the options.", properties: ["scale": S.string("Scale name."), "root": S.ref("Root, e.g. \"D\" or 2."), "chord": S.string("Chord name (single = off)."), "chordSet": ["type": ["string", "null"], "description": "Chord set id, or \"\" to turn off."], "scaleMode": S.string("Mode.", ["snap", "degrees"]), "octave": S.integer("Keyboard octave.", minimum: 0, maximum: 8)], required: []),
        MCPTool(name: "transport", method: "transport", description: "Play/stop. action: play (current mode), playPattern, playSong, stop, status. Options: pattern, row (start row), block (song block to start from), loop {from, to} rows for pattern playback (null clears). Mind the user's ears: ask before playing.", properties: ["action": S.string("Action.", ["play", "playPattern", "playSong", "stop", "status"]), "pattern": S.pattern, "row": S.integer("Start row.", minimum: 0), "block": S.integer("Song block.", minimum: 0), "loop": ["type": ["object", "null"], "properties": ["from": S.integer("First row.", minimum: 0), "to": S.integer("Last row.", minimum: 0)], "description": "Loop range for pattern playback."]], required: ["action"]),
        MCPTool(name: "undo", method: "undo", description: "Undo the last edit(s) — yours (labelled \"AI: …\") or the user's — exactly like ⌘Z.", properties: ["steps": S.integer("How many steps (default 1).", minimum: 1, maximum: 90)], required: []),
        MCPTool(name: "redo", method: "redo", description: "Redo, like ⌘⇧Z.", properties: ["steps": S.integer("How many steps (default 1).", minimum: 1, maximum: 90)], required: []),
        MCPTool(name: "get_history", method: "history", description: "Recent undo and redo labels.", properties: ["limit": S.integer("Max labels per list (default 20).", minimum: 1, maximum: 90)], required: []),
        MCPTool(name: "save_project", method: "save", description: "Save the project to its file, or to path (absolute, .supertracker). Unsaved sessions need a path.", properties: ["path": S.string("Absolute path ending in .supertracker.")], required: []),
        MCPTool(name: "export_audio", method: "exportAudio", description: "Render the song or one pattern to a WAV file (offline, silent).", properties: ["path": S.string("Absolute path ending in .wav."), "mode": S.string("What to render.", ["song", "pattern"]), "pattern": S.pattern, "tail": S.number("Effect tail in seconds (0–30, default 3).")], required: ["path"]),
        MCPTool(name: "list_commands", method: "listCommands", description: "List command-palette (⌘K) commands with ids.", properties: ["query": S.string("Filter words.")], required: []),
        MCPTool(name: "run_command", method: "runCommand", description: "Run a command-palette command by id; it acts on the current view/selection (use show_in_app first). Commands that open dialogs wait for the user.", properties: ["id": S.string("Command id from list_commands.")], required: ["id"]),
        MCPTool(name: "show_in_app", method: "show", description: "Point the user's view at something (no undo step): view (pattern, instrument, browser, mixer, automation, song), pattern, track, row, column (note/instrument/velocity/fx1/fx2), instrument, select {fromTrack,toTrack,fromRow,toRow} or null, channel (selects a mixer channel; shows the mixer unless view is given).", properties: ["view": S.string("View.", ["pattern", "instrument", "browser", "mixer", "automation", "song"]), "pattern": S.pattern, "track": S.track, "row": S.integer("Row.", minimum: 0), "column": S.ref("Column name or 0–4."), "instrument": S.instrument, "select": ["type": ["object", "null"], "description": "Selection rectangle.", "properties": ["fromTrack": S.track, "toTrack": S.track, "fromRow": S.integer("First row.", minimum: 0), "toRow": S.integer("Last row.", minimum: 0)]], "channel": S.channel], required: []),
        MCPTool(name: "call_api", method: "*", description: "Call any STAgent API method directly (advanced). Use method \"help\" to list every method and its parameters.", properties: ["method": S.string("API method name, e.g. \"help\", \"readPattern\"."), "params": S.object("Method parameters.")], required: ["method"]),
    ]
    static let byName: [String: MCPTool] = Dictionary(uniqueKeysWithValues: tools.map { ($0.name, $0) })
}

final class MCPServer {
    static let supportedVersions = ["2025-06-18", "2025-03-26", "2024-11-05"]
    static let instructions = """
    SuperTracker is a tracker DAW running on the user's Mac. These tools read and edit the song that is open right now; every edit appears in the app immediately and is one ⌘Z undo step labelled "AI: …". Start with get_song (and read_pattern / screenshot to see the music). Tracks, instruments and rows are 0-based; notes use names like C-4 (= MIDI 60). Buses, the master channel, outputs and sends: get_mixer (address them with channel: "master", a bus name or "bus:<id>"). Do not start playback unless the user asks.
    """
    typealias Backend = (_ method: String, _ params: [String: Any]) throws -> (result: Any, notices: [String])
    private let backend: Backend
    private(set) var initialized = false

    init(backend: @escaping Backend) { self.backend = backend }

    /// Handles one JSON-RPC message; returns the response, or nil for notifications.
    func handle(_ message: [String: Any]) -> [String: Any]? {
        let id = message["id"]
        guard let method = message["method"] as? String else {
            if id != nil, message["result"] != nil || message["error"] != nil { return nil } // a response to us; ignore
            return AgentControl.errorResponse(id: id, code: -32600, message: "Invalid request")
        }
        let isNotification = id == nil || id is NSNull
        let params: [String: Any]
        do { params = try AgentControl.namedObject(message["params"], field: "params") }
        catch { return isNotification ? nil : AgentControl.errorResponse(id: id, code: -32602, message: error.localizedDescription) }
        switch method {
        case "initialize":
            let requested = params["protocolVersion"] as? String ?? Self.supportedVersions[0]
            initialized = true
            return success(id, [
                "protocolVersion": Self.supportedVersions.contains(requested) ? requested : Self.supportedVersions[0],
                "capabilities": ["tools": ["listChanged": false]],
                "serverInfo": ["name": "supertracker", "title": "SuperTracker", "version": "0.1.0"],
                "instructions": Self.instructions
            ])
        case "notifications/initialized", "notifications/cancelled", "notifications/roots/list_changed": return nil
        case "ping": return isNotification ? nil : success(id, [:])
        case "tools/list": return success(id, ["tools": MCPCatalogue.tools.map(\.definition)])
        case "resources/list": return success(id, ["resources": []])
        case "resources/templates/list": return success(id, ["resourceTemplates": []])
        case "prompts/list": return success(id, ["prompts": []])
        case "tools/call":
            guard let name = params["name"] as? String, let tool = MCPCatalogue.byName[name] else {
                return AgentControl.errorResponse(id: id, code: -32602, message: "Unknown tool: \(params["name"] as? String ?? "(none)")")
            }
            do {
                return success(id, callTool(tool, arguments: try AgentControl.namedObject(params["arguments"], field: "arguments")))
            } catch { return AgentControl.errorResponse(id: id, code: -32602, message: error.localizedDescription) }
        default:
            return isNotification ? nil : AgentControl.errorResponse(id: id, code: -32601, message: "Method not found: \(method)")
        }
    }

    func callTool(_ tool: MCPTool, arguments: [String: Any]) -> [String: Any] {
        var method = tool.method, params = arguments
        if tool.method == "*" {
            guard let name = arguments["method"] as? String, !name.isEmpty, !name.hasPrefix("app.") || name == "app.ping" else { return errorResult("call_api needs method: an STAgent method name (try \"help\").") }
            method = name
            do { params = try AgentControl.namedObject(arguments["params"], field: "params") }
            catch { return errorResult(error.localizedDescription) }
        }
        do {
            let (result, notices) = try backend(method, params)
            if tool.method == "app.screenshot", let image = result as? [String: Any], let data = image["data"] as? String {
                let size = "\(image["width"] ?? "?")×\(image["height"] ?? "?")"
                return ["content": [["type": "image", "data": data, "mimeType": image["mimeType"] as? String ?? "image/png"], ["type": "text", "text": "SuperTracker window, \(size) px."]]]
            }
            return ["content": [["type": "text", "text": Self.render(result, notices: notices)]]]
        } catch {
            return errorResult(error.localizedDescription)
        }
    }

    private func errorResult(_ message: String) -> [String: Any] { ["content": [["type": "text", "text": message]], "isError": true] }
    private func success(_ id: Any?, _ result: [String: Any]) -> [String: Any] { ["jsonrpc": "2.0", "id": id ?? NSNull(), "result": result] }

    /// Compact JSON for the model; tracker grids are shown as plain text lines.
    static func render(_ result: Any, notices: [String]) -> String {
        var text = ""
        if var object = result as? [String: Any], let lines = object["lines"] as? [String] {
            let header = object["header"] as? String
            object["lines"] = nil; object["header"] = nil
            text = ([header].compactMap { $0 } + lines).joined(separator: "\n") + "\n" + json(object)
        } else { text = json(result) }
        if !notices.isEmpty { text += "\nApp messages: " + notices.joined(separator: " | ") }
        return text
    }

    static func json(_ value: Any) -> String {
        if let string = value as? String { return string }
        return CompactJSON.string(value)
    }

    /// Responses to one stdio line: a message, or a batch (an empty batch, or a non-object entry, is an Invalid Request).
    func responses(toLine line: String) -> [[String: Any]] {
        guard let data = line.data(using: .utf8), let object = try? JSONSerialization.jsonObject(with: data) else {
            return [AgentControl.errorResponse(id: nil, code: -32700, message: "Parse error")]
        }
        let invalid = AgentControl.errorResponse(id: nil, code: -32600, message: "Invalid request")
        if let message = object as? [String: Any] { return handle(message).map { [$0] } ?? [] }
        guard let batch = object as? [Any], !batch.isEmpty else { return [invalid] }
        return batch.compactMap { ($0 as? [String: Any]).map(handle) ?? invalid }
    }

    // MARK: - stdio

    /// Runs the MCP stdio loop until stdin closes. Logs go to stderr only.
    static func runStdio(socketPath: String = AgentControl.socketPath()) -> Int32 {
        signal(SIGPIPE, SIG_IGN)
        setvbuf(stdout, nil, _IOFBF, 1 << 16)
        let client = AgentControlClient(path: socketPath)
        let server = MCPServer { method, params in try client.call(method, params: params) }
        fputs("SuperTracker MCP server ready (control socket \(socketPath)).\n", stderr)
        while let line = readLine(strippingNewline: true) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty { continue }
            for response in server.responses(toLine: trimmed) { write(response) }
        }
        client.disconnect()
        return 0
    }

    private static func write(_ message: [String: Any]) {
        guard let data = try? AgentControl.encode(message) else { return }
        data.withUnsafeBytes { raw in _ = fwrite(raw.baseAddress, 1, raw.count, stdout) }
        fflush(stdout)
    }

    /// `SuperTracker --ctl <method> ['<json params>']`: prints the JSON result; exit 1 on error.
    static func runControl(arguments: [String], socketPath: String = AgentControl.socketPath()) -> Int32 {
        signal(SIGPIPE, SIG_IGN)
        guard let method = arguments.first else {
            fputs("Usage: SuperTracker --ctl <method> ['<json params>']\nExample: SuperTracker --ctl summary\n         SuperTracker --ctl writeSteps '{\"steps\":[{\"track\":0,\"row\":0,\"note\":\"C-4\"}]}'\n         SuperTracker --ctl help\n", stderr)
            return 64
        }
        var params: [String: Any] = [:]
        if arguments.count > 1 {
            guard let data = arguments[1].data(using: .utf8), let object = try? JSONSerialization.jsonObject(with: data), let dictionary = object as? [String: Any] else {
                fputs("The params must be a JSON object, e.g. '{\"pattern\":0}'.\n", stderr); return 64
            }
            params = dictionary
        }
        do {
            let (result, notices) = try AgentControlClient(path: socketPath).call(method, params: params)
            var output: Any = result
            if !notices.isEmpty { output = ["result": result, "notices": notices] }
            print(CompactJSON.string(output, pretty: true))
            return 0
        } catch {
            fputs((error.localizedDescription) + "\n", stderr)
            return 1
        }
    }
}

/// JSON text with shortest round-trip numbers (0.8, not 0.80000000000000004) and sorted keys,
/// for tool results the model reads and for `--ctl` output.
enum CompactJSON {
    static func string(_ value: Any, pretty: Bool = false) -> String {
        var out = ""
        write(value, into: &out, pretty: pretty, indent: 0)
        return out
    }

    private static func write(_ value: Any, into out: inout String, pretty: Bool, indent: Int) {
        let newline = pretty ? "\n" + String(repeating: "  ", count: indent + 1) : ""
        let close = pretty ? "\n" + String(repeating: "  ", count: indent) : ""
        switch value {
        case is NSNull: out += "null"
        case let number as NSNumber:
            if CFGetTypeID(number) == CFBooleanGetTypeID() { out += number.boolValue ? "true" : "false" }
            else if CFNumberIsFloatType(number) {
                let double = number.doubleValue
                if !double.isFinite { out += "null" }
                else if double == double.rounded(), abs(double) < 1e15 { out += String(Int64(double)) }
                else { out += "\(double)" }
            } else { out += number.stringValue }
        case let string as String: escape(string, into: &out)
        case let array as [Any]:
            if array.isEmpty { out += "[]"; return }
            out += "["
            for (index, item) in array.enumerated() { out += (index > 0 ? "," : "") + newline; write(item, into: &out, pretty: pretty, indent: indent + 1) }
            out += close + "]"
        case let dictionary as [String: Any]:
            if dictionary.isEmpty { out += "{}"; return }
            out += "{"
            for (index, key) in dictionary.keys.sorted().enumerated() {
                out += (index > 0 ? "," : "") + newline
                escape(key, into: &out); out += pretty ? ": " : ":"
                write(dictionary[key]!, into: &out, pretty: pretty, indent: indent + 1)
            }
            out += close + "}"
        default: escape(String(describing: value), into: &out)
        }
    }

    private static func escape(_ string: String, into out: inout String) {
        out += "\""
        for scalar in string.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if scalar.value < 0x20 { out += String(format: "\\u%04x", scalar.value) } else { out.unicodeScalars.append(scalar) }
            }
        }
        out += "\""
    }
}
