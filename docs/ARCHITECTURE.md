# SuperTracker implementation contract

Native macOS Swift executable, AppKit + WKWebView UI, original C++ DSP exposed through C functions, AVAudioEngine as device/plugin graph. No JUCE or external build dependency. macOS 14+, Swift language mode 5.

This document records the original integration contract and selected current interfaces; schema examples are illustrative subsets. The implementation and [feature status](FEATURE_STATUS.md) define current capabilities and tested boundaries.

## Files and ownership

- Root: Package.swift, App.swift, Bridge.swift, project persistence, build/package scripts, integration.
- Engine agent: Sources/TrackerEngine/**, tests/engine_tests.cpp.
- Audio agent: Sources/SuperTracker/AudioHost.swift (AU, CoreMIDI, recording, bounce if possible).
- Library agent: Sources/SuperTracker/LibraryService.swift (async scanning, metadata, NKS parsing).
- UI agent: Resources/Web/** (HTML, CSS, JS and UI state).
- Research agent: REFERENCE.md.

## Bridge

Web UI calls `window.webkit.messageHandlers.supertracker.postMessage({id,command,payload})`.
Native response invokes `window.nativeReceive({id,ok,result,error})`.
Native unsolicited notifications invoke `window.nativeEvent({type,payload})`.
JS should expose `window.nativeReceive` and `window.nativeEvent` early, and wrap bridge in Promise `request(command,payload)`.

Commands: `ready`, `project.new`, `project.open`, `project.save`, `project.saveAs`, `project.autosave`, `engine.project`, `transport.play`, `transport.stop`, `transport.seek`, `note.on`, `note.off`, `engine.panic`, `sample.load`, `sample.preview`, `sample.stopPreview`, `sample.edit`, `library.scan`, `library.search`, `library.roots`, `library.chooseRoot`, `library.favorite`, `plugins.list`, `plugin.load`, `plugin.remove`, `plugin.editor`, `plugin.parameters`, `plugin.parameter`, `plugin.state`, `plugin.preset`, `audio.devices`, `audio.configure`, `audio.record`, `audio.stopRecord`, `audio.export`, `midi.devices`, `midi.configure`, `dialog.openSample`, `dialog.export`, `app.external`.

Ready result `{project,dirty,roots,plugins,audio,midi,version,warning?,openPath?}`. Finder requests before readiness return as `openPath`; later requests emit `project.openRequested`. The UI applies its Save/Discard/Cancel transition guard before replacing a session. Project and transport mutations should return useful state. Status event at 20 Hz `{playing,row,pattern,sequence,peakL,peakR,trackPeaks:[...],cpu}`. Library scan progress event `{scanning,count,root}`. Errors shown in UI. Noninteractive requests use bounded waits and understandable errors; native file dialogs and permission panels remain pending until the user completes or cancels them. Every UI control must be backed by implementation; unsupported cases explain a limitation.

Missing-media repair commands are `project.media.inspect`, `project.media.search`, `project.media.apply`, `project.media.cancel` and `project.media.chooseRoot`. Bridge validates the current project snapshot and delegates filesystem work to MediaRelinkService, which bounds/coalesces searches and emits `project.media.progress`. Applying a proposal returns project data for the UI undo transaction. `project.importMIDI` and `project.exportMIDI` handle Standard MIDI Files.

## Project v1 schema

`{version:1,name,bpm:120,linesPerBeat:4,swing:0,masterVolume:0.7,activePattern:0,tracks:[...],instruments:[...],patterns:[...],sequence:[0],loopSong:false,delay:{time:0.375,feedback:0.35,mix:0.3},reverb:{size:0.7,damping:0.4,mix:0.25}}`

Track `{id:0,name,color,volume:0.8,pan:0,mute:false,solo:false,instrument:0,delaySend:0,reverbSend:0,plugins:[]}`. Any number of tracks (1..1024; 1024 is only a sanity check). Engine tables, mixer strips and bus caches are sized from the project; the native return bus is channel max(16, trackCount) (`st_return_bus`). Grouping adds `groupID`, `pad` (0–15), `padNote`, `chokeGroup` (0–16, scoped to the group) and `voiceMode` (`poly`/`mono`/`legato`) to a track.

Group `{id,name,color,collapsed,mixerCollapsed,busID,libraryPath?,importNotes?}` in `project.groups`. Members are contiguous tracks; the group bus is `routing.buses[] {id:busID,type:'group',groupID}` and members output to it. A kit is a group ([KIT_GROUPS_DESIGN.md](KIT_GROUPS_DESIGN.md)).

Instrument `{id:0,name,type:'synth'|'sample'|'au',waveform:'sine'|'saw'|'square'|'triangle'|'noise',samplePath:'',rootNote:60,volume:0.7,pan:0,tune:0,mode:'oneshot'|'forward'|'reverse'|'pingpong'|'slice'|'wavetable'|'granular',start:0,end:1,loopStart:0,loopEnd:1,attack:0.005,decay:0.2,sustain:0.7,release:0.2,cutoff:18000,resonance:0,filterType:'lowpass',drive:0,bitDepth:16,lfoRate:0,lfoDepth:0,sliceCount:16,grainSize:0.08,grainPosition:0.5,plugin:null}`. Up to 64 instruments. Native stores decoded sample paths internally; project retains reference. Edits create project-owned new files, never modify originals.

Instrument insert chain: `audioPlugins[]` entries with `owner:'instrument:<index>'`, `kind:'effect'`, `slot` 0–15 (built-ins, AU, VST3, VST2), edited on the Instrument page (`instrument-effects.js`, the mixer's `insert-chain.js` through `STInserts`). The host runs one instance per track whose assigned instrument it is, at the start of that track's strip, before the track's own inserts: instrument slot n is channel slot 16 + n of the track's owner, instance ID `<instanceID>-on-<routingID>` (`InstrumentEffects` in `MixerChannel.swift`; `MixerChannel.chainOrder` puts those slots first, so consecutive built-ins of both chains share one insert-chain node). The engine mixes every voice a track plays into the track's channel, including steps that name another instrument, so the chain of the track's *assigned* instrument applies to the whole track; per-step instrument chains would need a channel per (track, instrument). `plugin.*` commands with an instrument owner fan edits out to every instance and read state, parameters and the editor from the first track's instance (`AudioHost+InstrumentEffects.swift`); snapshots report each effect once, owned by its instrument, and the next publication restores that state into the other instances. An instrument no track plays keeps its descriptors without instances. Automation lanes with `owner:'instrument:<n>'` and target `plugin:effect:<slot>:<address>` become one host lane per playing track; a step-FX lock naming an instrument effect's `instanceID` reaches the instance on the lock's track. Copies of an instrument (track duplicate, MK3 pad duplicate, kits) take its chain with fresh instance IDs; `.stkit` files keep it inside the Sound's `instrument.effects`. Freeze does not bake instrument chains yet.

Pattern `{id:0,name:'Pattern 01',length:64,steps:[track0steps,track1steps,...],automation:[...]}`. Each steps array indexed row. Step is null or `{note:60,instrument:0,velocity:100,gate:0.8,chord:[0,4,7],fx1:{type:'volume',value:0.5},fx2:null}`. note -1 means note-off, -2 cut. Missing fields take sensible defaults. FX types: volume, pan, pitch, cutoff, resonance, delay, reverb, probability (0..1), ratchet (1..8), reverse (0/1), offset (0..1), glide (semitones), microtiming (0..0.95 rows), arpeggio (integer semitone pattern optional).

Automation lane `{id,track:0,target:'volume'|'pan'|'cutoff'|'resonance'|'delay'|'reverb'|'pitch'|'plugin:instrument:slot:address'|'plugin:effect:slot:address',name,interpolation:'linear'|'step',points:[{row:0,value:0.5}]}`. Native targets values same units as associated parameters; plugin values normalized 0..1. UI orders automated targets above unused; searchable target chooser, context menu and keyboard shortcut to automate focused parameter.

## AudioHost Swift contract

`final class AudioHost` exposes `var onEvent: ((String,[String:Any])->Void)?`, `init() throws`, `func handle(_ command:String,payload:[String:Any]) async throws -> Any`, `func applyProject(_ project:[String:Any]) throws`, `func status() -> [String:Any]`, `func shutdown()`. Root initializes on main thread and routes engine.*, transport.*, note.*, sample.*, plugins.*, plugin.*, audio.*, midi.* to handle. Host may add helpers/files under Audio* / MIDI* only. Integration with engine C API agreed directly between agents. `applyProject` is responsible for passing all project settings/sequence/steps/automation to engine, and restoring plugins if specified. `sample.load` payload `{path,instrument}` loads audio and returns `{path,name,duration,sampleRate,channels,peaks:[...]}`. Project loading reloads referenced samples. `audio.export` payload `{path,mode:'song'|'pattern'|'stems',tail:3}`.

## LibraryService Swift contract

`final class LibraryService` exposes `var onEvent: ((String,[String:Any])->Void)?`, `init()`, `func handle(_ command:String,payload:[String:Any]) async throws -> Any`. `library.roots` returns `[{path,name,available}]`. scan payload `{roots:[path]}` (default discovered roots); returns summary quickly and emits progress/results event. Search payload `{query,kind:'all'|'sample'|'nks',category,limit:200,offset:0,favorites:false}` returns `{items:[{id,path,name,kind,category,tags:[String],pack,bpm?,key?,favorite}],total,scanning}`. favorite payload `{path,favorite}`. `library.nks` payload `{path}` returns parsed metadata/controller maps/plugin ID/state availability honestly. `library.chooseRoot` handled by root and registered through scan.

## Packaging

`swift build` builds native binary. scripts/build.sh copies binary, Resources/Web and Info.plist into build/SuperTracker.app. No installation outside repo is necessary. `scripts/test.sh` runs regression suites; `scripts/test.sh --native` builds and invokes the application with `--self-test` to render native integration fixtures. The service-dependent virtual CoreMIDI test has a separate recipe in [MIDI_FILES.md](MIDI_FILES.md#virtual-port-verification). App loads Web/index.html from bundle, dev fallback from cwd. Native menus implement standard macOS shortcuts and dispatch JS commands.
