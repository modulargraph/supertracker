# End-to-end tests of the Rust host

These run the real interface (`ui`) against the real host (`crates/st-app`), not against a
stand-in. They need the executable to be built first:

```sh
cargo build -p st-app
```

Both scripts take the path of `supertracker` as their argument; without one they use
`$SUPERTRACKER_BIN`, then `$CARGO_TARGET_DIR/debug/supertracker`, then `target/debug/supertracker`.
Each run works in a temporary folder (its own support folder through `SUPERTRACKER_SUPPORT_DIR`) and
removes it.

## `run.js`: the page in Chromium, the host with `--serve`

```sh
node tests/host_e2e/run.js
```

Needs Node 22 and Playwright with Chromium (`require('playwright')`). It starts
`supertracker --serve --dialog-answers <file>`, opens the URL the host prints, and checks that:

1. the host prints `http://127.0.0.1:<port>/?token=<32 hex digits>`; files outside the interface
   folder are not served; the bridge refuses a wrong token and a client that is not the served page;
2. the page boots against the host over the WebSocket (`state.native`, transport `websocket`, the
   host's version and command list), not in browser-preview mode;
3. a note entered in the page is saved to a file through `project.save`, whose Save chooser is answered
   from the answers file; the file on disk contains the note;
4. a command the host has not ported yet (the test takes one that is missing from `ready.commands`)
   is refused with "… is not implemented yet …" and later requests still work;
5. after a reload the page gets the session back; New clears it; `project.open` through the (scripted)
   Open chooser brings the note back;
6. `quit` on the host's standard input ends it with status 0, and the page reports the lost connection
   instead of waiting;
7. a second host process restores the session from the recovery copy, with its file and the settings;
8. audio: the page loads the demo song and presses Play; the position advances from the host's real
   `status` events and the host reports non-silent master and track peaks; a note edited while playing
   reaches the engine (`engine.snapshot`) without stopping playback or raising an error; Stop stops;
9. the library: in the Browser workspace, Add folder (the scripted folder chooser answers with a folder
   of WAV files the test generated) adds a library root and scans it; searches show its sounds in the
   result list; a sound is made a favorite with its star. After the host was restarted (check 7) the
   Browser finds the sounds again from the index cache and the star is still set;
10. AI control: `supertracker --ctl summary` and an MCP session over `supertracker --mcp` (initialize,
    tools/list, `get_song`, `write_steps`, `read_pattern`) reach the page through the host's control
    socket; the written steps are in the page's project as one undo step each, and the page shows its
    "AI connected" badge. Unchecking "Allow AI control" in Settings disconnects the clients (the MCP
    tool answers with an error, `--ctl` exits with 1); checking it again lets the same MCP session
    reconnect;
11. SIGTERM, SIGINT and SIGHUP each end a host with status 128 + the signal's number, after the
    recovery copy was written and the control socket closed and removed (the same shutdown as Quit,
    which also leaves a Maschine MK3 dark and silences external MIDI instruments).

The host runs with `SUPERTRACKER_AUDIO_DEVICE=null` (the null audio device renders in real time to
nowhere) and `SUPERTRACKER_MIDI=off` (no MIDI device is opened), so the run is the same on every
machine. The control socket follows the run's support folder, so a run never meets an application
that is open on the same machine.

Plug-ins (after the audio checks): the run needs `supertracker-plugin-container` and the CLAP test
plug-in of `crates/plugin-host` next to the host executable and builds them there when they are
missing (`cargo build --manifest-path crates/plugin-host/Cargo.toml -p stpc-container -p
stpc-clap-test-plugin` with the same target directory). It then checks that a folder holding the
plug-in is scanned by container processes (`plugins.scan`) and listed with the built-in effects
without the plug-in being mapped into the host (`/proc/<pid>/maps`); that the mixer's "Add
plug-in" loads it onto the busiest track of the demo song and puts its descriptor with its state
into the project; that its gain is heard on that track's meter (zero is silence, half brings it
back) while the song plays on; and that Save As stores the state captured at save, an unsaved edit
of the plug-in is noticed before New (the Discard question), and opening the file again restores
the plug-in with the saved state.

## `window_smoke.sh`: the window in a virtual display (Linux)

```sh
tests/host_e2e/window_smoke.sh
```

Needs `xvfb-run`, `xdotool`, `node` and the GTK 3 / WebKitGTK 4.1 runtime. It starts the window under
Xvfb with `SUPERTRACKER_TRACE=1` and drives it with synthetic keys and clicks: handshake over the web
view's channel, a menu shortcut, a native context menu, the native file chooser, and Quit through the
handshake with the page. For these it reads the bridge trace.

It also asks the window for the screenshots AI clients get, through `supertracker --ctl
app.screenshot` and an MCP session over `supertracker --mcp` (`window_screenshot.js`, which decodes
the PNG files itself): the picture has the size of the web view (the window without its menu bar),
a requested `width` scales it down with its proportions kept and a larger one does not enlarge it,
it has the interface in it (many colours, none of them on most of the pixels), and a picture taken
with the command palette open differs from the one before. A request made while the file chooser is
open must fail with a message after ten seconds, and a host started with `--serve` must refuse with
a message. `SUPERTRACKER_SMOKE_PICTURES=<folder>` keeps the pictures for a look.

## Scripted file choosers

A test cannot click in a file chooser. `--dialog-answers <file>` (or `SUPERTRACKER_DIALOG_ANSWERS`)
makes the host answer choosers from a JSON array instead of showing them:

```json
["/tmp/run/Song.supertracker", null, "/tmp/run/Song.supertracker"]
```

Each chooser takes the next element: a string is the chosen path, `null` is Cancel. The file is read
again for every chooser, so a test can append answers while the host runs; when none is left, the
command that needed the chooser fails with a message that says so.
