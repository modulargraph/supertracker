// Pure control-state tests; no MIDI client, audio engine, device or plug-in.
// swiftc -O -swift-version 5 Sources/SuperTracker/MIDISustainState.swift \
//   tests/MIDISustainStateTests.swift -o /tmp/midi-sustain-tests && /tmp/midi-sustain-tests
import Foundation

@main enum MIDISustainStateTests {
    typealias Note = MIDISustainState.Destination
    static func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw NSError(domain: "MIDISustainStateTests: " + message, code: 1) }
    }
    static func main() throws {
        do {
            var state = MIDISustainState()
            state.setPedal(source: 10, channel: 1, down: true)
            try check(state.deferNoteOff(source: 10, channel: 1, track: 0, note: 60), "A must sustain")
            try check(!state.deferNoteOff(source: 20, channel: 1, track: 1, note: 64), "A must not sustain source B")
            try check(state.setPedal(source: 20, channel: 1, down: false).isEmpty, "B pedal-up must not release A")
            try check(state.isDown(source: 10, channel: 1), "B must not disarm A")
            try check(state.setPedal(source: 10, channel: 1, down: false) == [Note(track: 0, note: 60)], "A exact release")
            print("PASS input-source isolation and unrelated pedal-up")
        }
        do {
            var state = MIDISustainState()
            state.setPedal(source: 10, channel: 1, down: true)
            try check(!state.deferNoteOff(source: 10, channel: 2, track: 0, note: 60), "Channel 1 must not sustain channel 2")
            state.setPedal(source: 10, channel: 2, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 2, note: 60)
            state.deferNoteOff(source: 10, channel: 2, track: 3, note: 60)
            try check(state.setPedal(source: 10, channel: 2, down: false) == [Note(track: 3, note: 60)], "Channel 2 exact release")
            try check(state.setPedal(source: 10, channel: 1, down: false) == [Note(track: 2, note: 60)], "Channel 1 survived")
            print("PASS channel isolation on the same input source")
        }
        do {
            var state = MIDISustainState()
            state.setPedal(source: 10, channel: 1, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 1, note: 60)
            state.deferNoteOff(source: 10, channel: 1, track: 1, note: 62)
            state.deferNoteOff(source: 10, channel: 1, track: 2, note: 60)
            state.retrigger(source: 10, channel: 1, track: 2, note: 60)
            state.retrigger(source: 20, channel: 1, track: 1, note: 60)
            state.retrigger(source: 10, channel: 2, track: 1, note: 62)
            try check(state.setPedal(source: 10, channel: 1, down: false) == [Note(track: 1, note: 60), Note(track: 1, note: 62)], "Retrigger cancels only exact input/track/note")
            print("PASS exact retrigger cancellation preserves old target and other keys")
        }
        do {
            var state = MIDISustainState()
            state.setPedal(source: 10, channel: 1, down: true)
            // Emulate the required Host ordering: the note-off owns its earlier
            // destination even though the selected target now accepts channel 2.
            let original = Note(track: 4, note: 67), selectedChannel = 2
            try check(state.deferNoteOff(source: 10, channel: 1, track: original.track, note: original.note), "Owned off precedes new filter")
            let release = state.setPedal(source: 10, channel: 1, down: false)
            let acceptedByNewFilter = selectedChannel == 1
            try check(!acceptedByNewFilter && release == [original], "Pedal release precedes rejecting current filter")
            try check(!state.isDown(source: 10, channel: 1), "Old route pedal reset")
            print("PASS captured destination survives selection/filter changes (required Host ordering model)")
        }
        do {
            var state = MIDISustainState()
            state.setPedal(source: 10, channel: 1, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 0, note: 60)
            state.setPedal(source: 10, channel: 1, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 0, note: 60)
            try check(state.setPedal(source: 10, channel: 1, down: false) == [Note(track: 0, note: 60)], "Repeated messages must neither lose nor duplicate off")
            try check(state.setPedal(source: 10, channel: 1, down: false).isEmpty, "Repeated pedal-up inert")
            state.setPedal(source: 10, channel: 1, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 0, note: 60)
            state.retrigger(source: 10, channel: 1, track: 0, note: 60)
            try check(state.isDown(source: 10, channel: 1), "Retrigger retains pedal state")
            try check(state.deferNoteOff(source: 10, channel: 1, track: 0, note: 60), "Retriggered key-up defers again")
            try check(state.releaseAll() == [Note(track: 0, note: 60)], "Retriggered release retained")
            print("PASS repeated controller messages and retrigger-release cycle")
        }
        do {
            var state = MIDISustainState()
            for (source, channel) in [(Int32(10), 1), (Int32(20), 2)] {
                state.setPedal(source: source, channel: channel, down: true)
                state.deferNoteOff(source: source, channel: channel, track: 2, note: 70)
                state.deferNoteOff(source: source, channel: channel, track: 0, note: 60)
            }
            try check(state.releaseAll() == [Note(track: 0, note: 60), Note(track: 2, note: 70)], "Teardown returns deduplicated native destinations")
            try check(state.releaseAll().isEmpty && !state.isDown(source: 10, channel: 1), "Teardown clears pedals")
            state.setPedal(source: 10, channel: 1, down: true)
            state.deferNoteOff(source: 10, channel: 1, track: 1, note: 64)
            state.clear(); state.clear()
            try check(state.releaseAll().isEmpty && !state.isDown(source: 10, channel: 1), "Panic/reset never releases old state into new session")
            print("PASS route teardown, panic and idempotent clearing")
        }
        do {
            var state = MIDISustainState()
            for channel in [-1, 0, 17, Int.max] {
                state.setPedal(source: 10, channel: channel, down: true)
                try check(!state.isDown(source: 10, channel: channel), "Invalid channel must not arm")
                try check(!state.deferNoteOff(source: 10, channel: channel, track: 0, note: 60), "Invalid channel must not defer")
            }
            state.setPedal(source: Int32.min, channel: 16, down: true)
            for (track, note) in [(-1, 60), (16, 60), (0, -1), (0, 128)] {
                try check(!state.deferNoteOff(source: Int32.min, channel: 16, track: track, note: note), "Invalid native destination")
            }
            try check(state.deferNoteOff(source: Int32.min, channel: 16, track: 15, note: 127), "Signed CoreMIDI ID and maximum destination valid")
            try check(state.setPedal(source: Int32.min, channel: 16, down: false) == [Note(track: 15, note: 127)], "Boundary values preserved")
            print("PASS input validation and exact boundary values")
        }
        do {
            var state = MIDISustainState(destinationCount: 80)
            state.setPedal(source: 10, channel: 1, down: true)
            for track in [0, 15, 16, 31, 64, 79] {
                try check(state.deferNoteOff(source: 10, channel: 1, track: track, note: 60), "Ordinary and Kit destinations must sustain")
            }
            try check(!state.deferNoteOff(source: 10, channel: 1, track: 80, note: 60), "No channel beyond engine capacity")
            state.retrigger(source: 10, channel: 1, track: 79, note: 60)
            try check(state.setPedal(source: 10, channel: 1, down: false) == [0,15,16,31,64].map { Note(track: $0, note: 60) }, "Kit pedal-up retains exact original destinations")
            state.setPedal(source: 20, channel: 16, down: true)
            try check(state.deferNoteOff(source: 20, channel: 16, track: 79, note: 127), "Last Sound retrigger release must defer")
            try check(state.releaseAll() == [Note(track: 79, note: 127)], "Route teardown releases last Sound exactly")
            print("PASS configured Kit destinations sustain, retrigger and release through channel 79")
        }
        do {
            // Kit pad reorder: deferred releases follow their Sound to its new channel; a removed Sound's are dropped.
            var state = MIDISustainState(destinationCount: 80)
            state.setPedal(source: 10, channel: 1, down: true)
            for (track, note) in [(16, 36), (17, 38), (18, 42), (3, 60)] { _ = state.deferNoteOff(source: 10, channel: 1, track: track, note: note) }
            state.remapTracks([16: 17, 17: 16, 18: nil])
            try check(state.setPedal(source: 10, channel: 1, down: false) == [Note(track: 3, note: 60), Note(track: 16, note: 38), Note(track: 17, note: 36)], "Pedal-up must release the moved Sounds' new channels only")
            print("PASS deferred releases follow moved Kit Sounds")
        }
        print("PASS 9 pure MIDI sustain groups; no Host, MIDI device or audio execution")
    }
}
