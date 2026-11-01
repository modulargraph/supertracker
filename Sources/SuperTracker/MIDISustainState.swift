/// Sustain ownership for serialized MIDI input delivery (currently the main
/// thread). This is control state, not a render-thread collection. It performs
/// no MIDI/audio I/O and does not own held-key or recording-capture state.
struct MIDISustainState {
    struct Destination: Hashable {
        let track: Int
        let note: Int
    }

    private struct Input: Hashable {
        let source: Int32
        let channel: Int
    }

    // Presence means the pedal is down, including before any key is released.
    // Destination is part of the key: moving to another track and retriggering
    // the same physical key must not forget a former track's deferred release.
    private var inputs: [Input: Set<Destination>] = [:]
    private let destinationCount: Int

    /// The Host supplies its engine channel count, including independent Kit Sounds.
    init(destinationCount: Int = 16) { self.destinationCount = max(0, destinationCount) }

    func isDown(source: Int32, channel: Int) -> Bool {
        inputs[Input(source: source, channel: channel)] != nil
    }

    /// Admit pedal-down after the current route's input filters. Process
    /// pedal-up BEFORE those filters: previously owned notes must still release
    /// after selection/input-channel changes. Returned destinations are exact
    /// original note-off targets, sorted for deterministic control delivery.
    @discardableResult
    mutating func setPedal(source: Int32, channel: Int, down: Bool) -> [Destination] {
        guard (1...16).contains(channel) else { return [] }
        let input = Input(source: source, channel: channel)
        if down {
            if inputs[input] == nil { inputs[input] = [] }
            return []
        }
        return ordered(inputs.removeValue(forKey: input) ?? [])
    }

    /// Return true only when this input's pedal owns the sounding note-off.
    /// The caller must still acknowledge/capture the physical key release;
    /// sustain delays sound release, not recorded key-release timing.
    @discardableResult
    mutating func deferNoteOff(source: Int32, channel: Int, track: Int, note: Int) -> Bool {
        guard (0..<destinationCount).contains(track), (0..<128).contains(note) else { return false }
        let input = Input(source: source, channel: channel)
        guard inputs[input] != nil else { return false }
        inputs[input]!.insert(Destination(track: track, note: note))
        return true
    }

    /// A new sounding note cancels only its own deferred note-off. Other
    /// sources, channels, tracks and pitches remain owned by their pedals.
    mutating func retrigger(source: Int32, channel: Int, track: Int, note: Int) {
        let input = Input(source: source, channel: channel)
        inputs[input]?.remove(Destination(track: track, note: note))
    }

    /// Track instruments moved channels (old -> new, nil = removed). Deferred releases follow their track,
    /// applied simultaneously so swaps work; a removed track's voices are already forgotten, so its releases are dropped.
    mutating func remapTracks(_ moves: [Int: Int?]) {
        for (input, notes) in inputs {
            inputs[input] = Set(notes.compactMap { held in
                guard let moved = moves[held.track] else { return held }
                return moved.flatMap { (0..<destinationCount).contains($0) ? Destination(track: $0, note: held.note) : nil }
            })
        }
    }

    /// Route teardown releases every deferred destination, once per native
    /// track/note, and resets all pedals. Held physical keys are caller-owned.
    mutating func releaseAll() -> [Destination] {
        var destinations = Set<Destination>()
        for notes in inputs.values { destinations.formUnion(notes) }
        inputs.removeAll()
        return ordered(destinations)
    }

    /// Use after an already-issued engine/external panic; do not emit old
    /// deferred note-offs into a replacement session.
    mutating func clear() { inputs.removeAll() }

    private func ordered(_ notes: Set<Destination>) -> [Destination] {
        notes.sorted { $0.track == $1.track ? $0.note < $1.note : $0.track < $1.track }
    }
}
