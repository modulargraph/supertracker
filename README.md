# SuperTracker

A keyboard-first music tracker and studio for macOS. Write patterns in a tracker grid, play chords and scales from the computer keyboard, and arrange, mix, automate and export the song in one native app. It hosts AU, VST3 and VST2 plug-ins and has its own C++ audio engine.

![A song in the Pattern view: tracker grid, the song's blocks on the left and an automation lane running across the song](docs/images/pattern-view.png)

- **Tracker patterns** with step effects (probability, ratchet, glide, plug-in parameter locks) and pattern tools, all from the keyboard.
- **Play musically:** scales, chords and chord sets, note repeat and an arpeggiator, also from a MIDI keyboard.
- **Sounds:** a built-in synth, sampler and drum synths, 34 effects, kits, and browsers for samples, NKS presets and plug-ins.
- **Arrange, mix and automate:** song arranger, mixer with groups, sends and sidechains, and automation you can draw across patterns.
- **Every action has a key**, a `Cmd+K` command palette entry and a line in the keyboard reference (`?`).

| Song arranger | Song scroll: the grid runs through the song |
| --- | --- |
| ![Song arranger](docs/images/song-arranger.png) | ![Song scroll across two patterns](docs/images/song-scroll.png) |

## Status

Version 0.1 is a first public preview for people who are happy to build a Mac app from source. It runs on **macOS 14 or later on Apple Silicon only**, is ad-hoc signed (not notarized), and the newest features have been tested with automated tests and offline renders rather than real hardware. See the [release notes](CHANGELOG.md) for what works and what doesn't yet. Bug reports with steps to reproduce are welcome.

## Build

You need Apple's command-line tools (`xcode-select --install`) or Xcode.

```sh
git clone --recurse-submodules https://github.com/modulargraph/supertracker.git && cd supertracker
./scripts/build.sh            # release build at build/SuperTracker.app
open build/SuperTracker.app
```

`./scripts/install.sh` installs it as `/Applications/SuperTracker.app`. In the app, `Cmd+K` → **Load Midnight transmission demo** opens a demo song; `?` shows every key. Tests: `./scripts/test.sh` ([details](docs/TESTING.md)).

## Documentation

- [User guide](docs/USER_GUIDE.md): every workspace and its keys.
- [Feature status](docs/FEATURE_STATUS.md): what is built and how it was verified.
- [Architecture](docs/ARCHITECTURE.md) and [audio engine](docs/AUDIO_ENGINE.md) for contributors.

## License

GPL-3.0-or-later; see [LICENSE](LICENSE). The Steinberg VST3 SDK in `Vendor/vst3sdk` (a git submodule) has its own MIT license.
