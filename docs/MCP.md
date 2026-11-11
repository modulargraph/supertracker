# AI control (MCP)

SuperTracker can be edited by Claude (or any MCP client) **while the app is open**. Every edit
appears in the interface immediately, reaches the audio engine like your own edits, and is one
⌘Z undo step labelled `AI: …` in Edit › Undo.

## How it works

```
Claude Code / Desktop ──stdio MCP──▶ SuperTracker --mcp ──Unix socket──▶ running SuperTracker app
                                     (no window, no audio)               window.STAgent (agent-api.js) → ST.mutate
```

- The app listens on a local Unix domain socket, readable only by your user (mode 0600, peer
  uid checked): `~/Library/Application Support/SuperTracker/control.sock`
  (or `$SUPERTRACKER_SUPPORT_DIR/control.sock`, or `$SUPERTRACKER_CONTROL_SOCKET`; paths too long
  for a socket fall back to `$(getconf DARWIN_USER_TEMP_DIR)SuperTracker-<hash>.sock`).
- Protocol: one JSON-RPC 2.0 request per line: `{"jsonrpc":"2.0","id":1,"method":"writeSteps","params":{…}}`.
  Methods are the `window.STAgent` API (`help` lists them) plus native `app.ping` and `app.screenshot`.
- Requests run one at a time, in the page. Nothing simulates key presses.
- **Settings › Allow AI control (MCP)** turns the socket on or off (on by default; the choice is
  stored in the `agent` section of `settings.json` in the support folder). The status bar shows **AI connected** while a
  client is connected and pulses while it edits.

## Set up in Claude Code

Build the app first (`scripts/build.sh`). Inside this repository, the project `.mcp.json` already
registers the server (`scripts/mcp.sh`, which starts the built executable with `--mcp`); Claude Code
asks once to approve it. To use it from any folder, name the executable:

```sh
# macOS
claude mcp add --scope user supertracker -- "$PWD/build/SuperTracker.app/Contents/MacOS/SuperTracker" --mcp
# Linux
claude mcp add --scope user supertracker -- "$PWD/build/supertracker/supertracker" --mcp
```

Open SuperTracker, then talk: "Read pattern 1 and write a darker bassline on track 0", "Make the hats
swing and humanize them", "Load an 808 kick sample on a new track", "Take a screenshot".

## Set up in Claude Desktop

Add to `claude_desktop_config.json` (on macOS in `~/Library/Application Support/Claude/`), with the path of your executable:

```json
{ "mcpServers": { "supertracker": { "command": "/Applications/SuperTracker.app/Contents/MacOS/SuperTracker", "args": ["--mcp"] } } }
```

If SuperTracker is not open, tools return "Open SuperTracker first".

## Scripting CLI

```sh
SuperTracker --ctl help
SuperTracker --ctl summary
SuperTracker --ctl readPattern '{"tracks":[0],"format":"grid"}'
SuperTracker --ctl writeSteps '{"steps":[{"track":0,"row":0,"note":"C-4","fx1":"probability 50%"}]}'
SuperTracker --ctl undo
```

Prints the JSON result; exits 1 with the error message on failure.

## Conventions

- Tracks and instruments: 0-based index (the UI shows 01 for track 0) or name. Patterns: id (see
  `get_song`) or name; default is the pattern in the editor. Rows are 0-based.
- Notes: `C-4` (= MIDI 60), `C4`, `C#4`, `Db3`, MIDI numbers, `OFF`, `CUT`. Chords: `notes: ["C-4","E-4","G-4"]`
  or `chord: [0,4,7]` / `"minor"`.
- Step FX: `probability 50%`, `ratchet 3`, `pitch -12`, `cutoff 800Hz`, `pan -30%`, `reverse`,
  `arpeggio 37` (+3 +7), `microtiming 25%`, `volume 80%`, `slice 4`, `glide -12`, `delay 40%`,
  `reverb 30%`, `offset 25%`, `resonance 50%`. Percent types take percent numbers.
- Values with units: `800 Hz`, `1.2 kHz`, `-6 dB`, `20 ms`, `50%`, `30% L`, `+7 st`, `C-3`.
- Mixer channels (`channel`): `"master"`, a bus name (`"Avenue Kit"`) or `"bus:<id>"`, or a track index or
  name. `get_mixer` lists them with their ids, outputs, sends and insert slots.
- Validation happens before anything changes; errors explain the valid range or options.

## Tools

| Tool | What it does |
|---|---|
| `get_song` | Overview: tempo, meter, tracks, instruments, patterns, song blocks, sends, performance, history |
| `get_ui_state` | View, dialog, cursor, selection, transport |
| `screenshot` | PNG of the window's interface (MCP image content); optional `width` scales it down. See [Screenshots](#screenshots) |
| `read_pattern` | Steps with readable notes/FX and automation; `format:"grid"` for tracker text |
| `write_steps` | Set/merge/clear steps (notes, chords, velocity, optional gate, instrument, FX). New notes have no gate: they play until the next note, `OFF` or `CUT` on their track; `gate: 0`/`null` removes a gate. Optional `column` (0–7) writes a note column (columns 1–7 hold note, instrument, velocity and gate; the row's FX stay in column 0); `read_pattern` lists extra columns under `columns` |
| `clear_steps`, `insert_rows`, `delete_rows` | Range edits |
| `transform_steps` | transpose, humanize, velocity, quantize, snapToScale, reverse, rotate, gate, instrument, removeFX |
| `create_pattern`, `duplicate_pattern`, `rename_pattern`, `delete_pattern`, `set_pattern_length` | Patterns |
| `edit_song_sequence` | Song blocks: set, insert, remove, repeats, replace, move, duplicate, loop, clearLoop |
| `add_track`, `edit_track` | Tracks: name, mute, solo, volume, pan, sends, instrument |
| `set_song_settings` | Name, BPM, swing, meter, lines per beat, master volume, loop, metronome |
| `get_instrument`, `create_instrument`, `edit_instrument` | Built-in synth/sampler/MIDI instruments, parameters by name with units |
| `search_library`, `load_sample` | Sample library search and loading |
| `list_plugins`, `load_plugin`, `get_plugin_parameters`, `set_plugin_parameters` | AU/VST3/CLAP/built-in instruments and effects, on tracks, buses and Master (`channel`) |
| `edit_insert` | Bypass, remove or reorder an insert on any channel |
| `get_mixer` | Mix model, Master, buses, every track's output/sends/inserts, sidechains, delay/reverb settings |
| `edit_channel` | Name, volume, pan, mute, solo and output of tracks, buses and Master (one or several channels) |
| `create_bus`, `remove_bus`, `set_send` | Group/return buses, track outputs into a bus, custom sends |
| `set_send_effects` | Built-in delay (time or tempo division, feedback, tone, level) and reverb settings |
| `search_nks`, `load_nks` | NKS presets |
| `list_automation_targets`, `set_automation`, `delete_automation` | Automation lanes of tracks, buses and Master (`channel`) |
| `set_performance` | Scale, root, chord, chord set, keyboard octave |
| `transport` | play / playPattern / playSong / stop / status, start row/block, loop range |
| `undo`, `redo`, `get_history` | History (shared with ⌘Z) |
| `save_project`, `export_audio` | Save (path for new sessions); render song/pattern to WAV |
| `list_commands`, `run_command` | Any command-palette command |
| `show_in_app` | Move the user's view/cursor/selection |
| `call_api` | Any `STAgent` method (`help` lists all) |

Notes: keyboard performance settings (`set_performance`) and navigation (`show_in_app`) are not
undo steps.

### Mixer and routing

The tools reuse the mixer's own paths, so the mixer updates live and every call is one undo step:

- Outputs, buses and sends go through the routing panel's transaction (`STRouting.transaction` in
  `routing.js`): the audio host validates and applies the change, and the committed project is the undo step.
  Outputs and sends that would form a feedback loop are refused with the valid destinations.
- Levels, pan, mute/solo and names are ordinary project edits, like the mixer faders. Track volume and Master
  go up to 1.5 gain (+3.5 dB), buses and sends up to 2 (+6 dB); Master has volume only.
- Bus and Master inserts load, bypass, move and remove through the mixer's insert API (`STInserts` in
  `studio.js`). Their parameters are read and set like track plug-ins.
- Bus and Master automation lanes are owner lanes (`{owner: "bus:<id>", target}`): `volume`, `pan` (not
  Master), `send:<id>` and `plugin:effect:<slot>:<address>`. Send lanes of tracks are owner lanes too.
- A track group's (kit's) bus is removed by dissolving the group; `remove_bus` says so.

```
get_mixer {}
load_plugin {"channel": "Avenue Kit", "plugin": "Reverb"}
set_plugin_parameters {"channel": "Avenue Kit", "slot": 3, "values": {"Mix": "25%"}}
edit_insert {"channel": "master", "slot": 0, "bypass": true}
edit_channel {"channel": ["Keys", "Pad"], "output": "Avenue Kit"}
edit_channel {"channel": "master", "volume": "-3 dB"}
create_bus {"name": "Space", "type": "return"}
set_send {"from": "Avenue Kit", "to": "Space", "gain": "-12 dB"}
set_automation {"channel": "Avenue Kit", "target": "volume", "points": [{"row": 0, "value": "-inf dB"}, {"row": 63, "value": "0 dB"}]}
set_send_effects {"delay": {"time": "1/8.", "feedback": "40%"}}
```

`set_plugin_parameters` reads the parameters back before it captures the plug-in's state and again in its
reply (`applied` per parameter, and a `warning` naming any value the plug-in did not keep): some plug-ins
apply a burst of writes late or drop some, and a state captured too early would bring the old values back.
Dropped values are written again, one at a time, before the capture.

Not reachable yet: sidechain connections (listed by `get_mixer`, edited in the mixer). Songs with a tempo map change their first tempo/meter through the timing editor.

## Screenshots

`screenshot` (control method `app.screenshot`) returns what the window's web view shows, as a PNG file:
MCP image content plus a line of text with its size, or from the command line
`{"mimeType": "image/png", "data": "<base64>", "width": …, "height": …}`:

```sh
supertracker --ctl app.screenshot '{"width": 800}'
```

- The picture is the interface. The menu bar, native menus, file choosers and plug-in editor windows
  are not in it.
- Without `width` the picture is as wide as the window's content in layout units (1500 pixels for
  the default window; not doubled on a high-density display). `width` (320 to 2560 pixels) is the
  widest picture wanted: a wider window is scaled down to it and keeps its proportions, a narrower
  one is not enlarged.
- The web view takes the picture on the window's thread. The request waits for that thread for at most
  ten seconds; while a native menu or dialog is open it is not served, and the tool fails with
  "The SuperTracker window did not produce a screenshot within 10 s. …".
- A host without a window (`supertracker --serve`, and Windows, where the window does not exist)
  answers "SuperTracker is running without a window (for example with --serve), so there is no
  window to take a screenshot of."

State: on Linux (WebKitGTK's snapshot, PNG written by cairo) this is tested in the real window under
Xvfb (`tests/host_e2e/window_smoke.sh`). On macOS (`WKWebView`'s
`takeSnapshotWithConfiguration:completionHandler:`, PNG written by AppKit) the code is type-checked
from Linux only; it has never run.

## Development

- API: `ui/agent-api.js` (tests: `tests/agent_api_tests.js`, `tests/agent_safety_tests.js`; mixer, routing
  and inserts on the real mixer modules: `tests/agent_mixer_tests.js`, which also checks that every MCP tool names an
  existing method).
- Host side: the crate `st-agent` (`server.rs` is the application's listener, `client.rs` and `framing.rs` the
  client and wire format, `mcp.rs` and `tools.json` the MCP server and its tool catalogue,
  `screenshot.rs` the reply of `app.screenshot`; tests in `crates/st-agent/tests/`). The browser
  test (`tests/host_e2e/run.js`) runs an MCP session against the page.
- Screenshots: the window layer (`crates/st-app/src/screenshot.rs`) installs what takes the picture
  with `Agent::set_screenshot_provider` (`crates/st-host/src/areas/agent.rs`); `st-host` has no window
  code. The window test (`tests/host_e2e/window_smoke.sh`) asks the real window for pictures.
