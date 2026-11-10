//! Channel-layout negotiation and channel adaptation. Platform independent (tested on
//! Linux); the macOS side asks the unit (`channelCapabilities`, `setFormat:error:`) and
//! moves the samples with the kernels at the end of this file.
//!
//! The host fixes the layout of an instance (channels per bus). An Audio Unit supports
//! some channel configurations and not others, so the backend
//!
//! 1. picks, in order of preference, the configurations to try for the main bus pair
//!    ([`candidates`]); the first one the unit accepts wins;
//! 2. adapts each bus between the host's channel count and the unit's ([`ChannelMap`]):
//!    equal counts are passed straight through (no copy); a mono unit on a wider host bus
//!    is fed the average of the host channels and its output is copied to every host
//!    channel; a wider unit on a mono host bus is fed the host channel on every input and
//!    its outputs are averaged; any other mismatch uses the channels both sides have,
//!    feeds silence to surplus unit inputs and leaves surplus host outputs silent.

/// One `[inputs, outputs]` pair of `AUAudioUnit.channelCapabilities` (`AUChannelInfo`).
///
/// `-1` on both sides: any count, the same on both. `-1` and `-2`: any counts,
/// independently. A count below `-2`: at most that many channels in total. `0` inputs: the
/// unit takes no audio input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    pub inputs: i32,
    pub outputs: i32,
}

/// Groups the flat number list of `channelCapabilities` into pairs (a trailing odd number
/// is ignored).
pub fn capabilities(flat: &[i32]) -> Vec<Capability> {
    flat.chunks_exact(2).map(|pair| Capability { inputs: pair[0], outputs: pair[1] }).collect()
}

fn side_allows(spec: i32, channels: u32) -> bool {
    match spec {
        -2 | -1 => true,
        spec if spec < -2 => channels <= spec.unsigned_abs(),
        spec => channels == spec as u32,
    }
}

impl Capability {
    pub fn allows(&self, inputs: u32, outputs: u32) -> bool {
        if self.inputs == -1 && self.outputs == -1 {
            return inputs == outputs;
        }
        side_allows(self.inputs, inputs) && side_allows(self.outputs, outputs)
    }

    fn allows_output(&self, outputs: u32) -> bool {
        side_allows(self.outputs, outputs)
    }
}

/// Whether the unit states it supports this configuration. `caps` is `None` when the unit
/// states nothing (then an effect is assumed to need equal counts, as the system
/// documents, and a unit without input anything). `has_input`: the unit has an input bus.
pub fn supports(caps: Option<&[Capability]>, has_input: bool, inputs: u32, outputs: u32) -> bool {
    match caps {
        None | Some([]) => !has_input || inputs == outputs,
        Some(caps) if has_input => caps.iter().any(|cap| cap.allows(inputs, outputs)),
        Some(caps) => caps.iter().any(|cap| cap.allows_output(outputs)),
    }
}

/// Most channels of one bus this backend configures.
pub const MAX_BUS_CHANNELS: u32 = 64;

/// The main-bus configurations `(inputs, outputs)` to try, best first. `want_in` and
/// `want_out` are the host's channel counts of input bus 0 and output bus 0; for a unit
/// without an input bus every candidate has 0 inputs.
///
/// Order: what the host asked for; then configurations the unit states it supports,
/// nearest to the request first (stereo before mono on a tie); last, configurations the
/// unit does not state but that cost nothing to try, because stated capabilities are
/// sometimes wrong.
pub fn candidates(caps: Option<&[Capability]>, has_input: bool, want_in: u32, want_out: u32) -> Vec<(u32, u32)> {
    let want_out = want_out.clamp(1, MAX_BUS_CHANNELS);
    let want_in = if has_input {
        if want_in == 0 {
            want_out
        } else {
            want_in.min(MAX_BUS_CHANNELS)
        }
    } else {
        0
    };
    let usual: Vec<(u32, u32)> = if has_input {
        vec![
            (want_in, want_out),
            (want_out, want_out),
            (want_in, want_in),
            (2, 2),
            (1, want_out),
            (1, 2),
            (1, 1),
            (2, 1),
        ]
    } else {
        vec![(0, want_out), (0, 2), (0, 1)]
    };
    // Fixed configurations the unit lists itself.
    let mut stated: Vec<(u32, u32)> = caps
        .unwrap_or(&[])
        .iter()
        .filter(|cap| cap.outputs >= 1 && cap.outputs as u32 <= MAX_BUS_CHANNELS)
        .filter_map(|cap| match (has_input, cap.inputs) {
            (false, _) => Some((0, cap.outputs as u32)),
            (true, inputs) if inputs >= 1 && inputs as u32 <= MAX_BUS_CHANNELS => {
                Some((inputs as u32, cap.outputs as u32))
            }
            (true, -1) | (true, -2) => Some((want_in, cap.outputs as u32)),
            _ => None,
        })
        .collect();
    let distance = |&(i, o): &(u32, u32)| {
        let d = |a: u32, b: u32| a.abs_diff(b);
        // Prefer keeping the output width, then the input width, then stereo.
        (d(o, want_out) * 4 + d(i, want_in), o != 2, i != 2)
    };
    stated.sort_by_key(distance);

    let mut list: Vec<(u32, u32)> = Vec::new();
    fn push(pair: (u32, u32), list: &mut Vec<(u32, u32)>) {
        if !list.contains(&pair) {
            list.push(pair);
        }
    }
    let first = usual[0];
    push(first, &mut list);
    for &pair in usual.iter().chain(stated.iter()) {
        if supports(caps, has_input, pair.0, pair.1) {
            push(pair, &mut list);
        }
    }
    for &pair in &usual {
        push(pair, &mut list);
    }
    list
}

/// Channel counts to try on a sidechain (auxiliary input) bus the host allotted
/// `host_channels` to: the same width, then stereo, then mono.
pub fn sidechain_candidates(host_channels: u32) -> Vec<u32> {
    let mut list = vec![host_channels.clamp(1, MAX_BUS_CHANNELS)];
    for channels in [2, 1] {
        if !list.contains(&channels) {
            list.push(channels);
        }
    }
    list
}

/// Where the samples of one channel come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The channel with this index on the other side.
    Channel(usize),
    /// The average of all channels on the other side.
    Average,
    Silence,
}

/// How one bus is adapted between the host's channel count and the unit's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChannelMap {
    /// Channels the host allotted to the bus (0: the host has no such bus).
    pub host: usize,
    /// Channels the unit's bus is configured to (0: the unit has no such bus, or it is
    /// disabled).
    pub unit: usize,
}

impl ChannelMap {
    pub fn new(host: u32, unit: u32) -> ChannelMap {
        ChannelMap { host: host as usize, unit: unit as usize }
    }

    /// Input bus: what the unit's channel `unit_channel` is fed with (host channels).
    pub fn input_source(&self, unit_channel: usize) -> Source {
        if self.host == 0 || unit_channel >= self.unit {
            Source::Silence
        } else if self.unit == 1 && self.host > 1 {
            Source::Average
        } else if self.host == 1 {
            Source::Channel(0)
        } else if unit_channel < self.host {
            Source::Channel(unit_channel)
        } else {
            Source::Silence
        }
    }

    /// Output bus: what the host's channel `host_channel` receives (unit channels).
    pub fn output_source(&self, host_channel: usize) -> Source {
        if self.unit == 0 || host_channel >= self.host {
            Source::Silence
        } else if self.host == 1 && self.unit > 1 {
            Source::Average
        } else if self.unit == 1 {
            Source::Channel(0)
        } else if host_channel < self.unit {
            Source::Channel(host_channel)
        } else {
            Source::Silence
        }
    }

    /// Input bus: the unit's channel can read the host's channel of the same index in
    /// place (no staging copy needed).
    pub fn input_is_direct(&self, channel: usize) -> bool {
        self.input_source(channel) == Source::Channel(channel)
    }

    /// Output bus: the unit's channel can render straight into the host's channel of the
    /// same index.
    pub fn output_is_direct(&self, channel: usize) -> bool {
        channel < self.unit && self.output_source(channel) == Source::Channel(channel)
    }

    /// Channels the plug-in uses of the channels the host allotted (what
    /// `BusInfo::channels` reports).
    pub fn used_host_channels(&self) -> u32 {
        self.host.min(self.unit) as u32
    }
}

// ---- sample kernels (allocation-free; used on the real-time thread) ----------------------

/// `dst += src`, over the shorter of the two.
pub fn accumulate(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += *s;
    }
}

/// `dst *= factor`.
pub fn scale(dst: &mut [f32], factor: f32) {
    for d in dst.iter_mut() {
        *d *= factor;
    }
}

/// `dst` = the average of `sources` (silence if there are none).
pub fn average(dst: &mut [f32], sources: &[&[f32]]) {
    dst.fill(0.0);
    for source in sources {
        accumulate(dst, source);
    }
    if sources.len() > 1 {
        scale(dst, 1.0 / sources.len() as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(flat: &[i32]) -> Vec<Capability> {
        capabilities(flat)
    }

    #[test]
    fn capability_pairs() {
        assert_eq!(
            caps(&[1, 1, 2, 2, 9]),
            [Capability { inputs: 1, outputs: 1 }, Capability { inputs: 2, outputs: 2 }]
        );
        let any_equal = Capability { inputs: -1, outputs: -1 };
        assert!(any_equal.allows(1, 1) && any_equal.allows(6, 6) && !any_equal.allows(1, 2));
        let any_any = Capability { inputs: -1, outputs: -2 };
        assert!(any_any.allows(1, 2) && any_any.allows(2, 1) && any_any.allows(5, 5));
        assert!(Capability { inputs: -2, outputs: -1 }.allows(1, 8));
        let any_to_stereo = Capability { inputs: -1, outputs: 2 };
        assert!(any_to_stereo.allows(1, 2) && any_to_stereo.allows(6, 2) && !any_to_stereo.allows(2, 1));
        let up_to_four = Capability { inputs: -4, outputs: 2 };
        assert!(up_to_four.allows(4, 2) && up_to_four.allows(1, 2) && !up_to_four.allows(5, 2));
        let instrument = Capability { inputs: 0, outputs: 2 };
        assert!(instrument.allows(0, 2) && !instrument.allows(2, 2) && !instrument.allows(0, 1));
    }

    #[test]
    fn stated_support() {
        // Nothing stated: effects need equal counts, sources take anything.
        assert!(supports(None, true, 2, 2) && !supports(None, true, 1, 2));
        assert!(supports(Some(&[]), true, 1, 1) && !supports(Some(&[]), true, 2, 1));
        assert!(supports(None, false, 0, 2) && supports(None, false, 0, 7));
        let stereo_or_mono = caps(&[1, 1, 2, 2]);
        assert!(supports(Some(&stereo_or_mono), true, 2, 2));
        assert!(supports(Some(&stereo_or_mono), true, 1, 1));
        assert!(!supports(Some(&stereo_or_mono), true, 1, 2));
        // A source's input side is irrelevant.
        let synth = caps(&[0, 2, 0, 1]);
        assert!(supports(Some(&synth), false, 0, 2) && supports(Some(&synth), false, 0, 1));
        assert!(!supports(Some(&synth), false, 0, 6));
    }

    #[test]
    fn the_request_comes_first_when_supported() {
        assert_eq!(candidates(None, true, 2, 2)[0], (2, 2));
        assert_eq!(candidates(Some(&caps(&[-1, -1])), true, 2, 2)[0], (2, 2));
        assert_eq!(candidates(Some(&caps(&[-1, -1])), true, 1, 1)[0], (1, 1));
        assert_eq!(candidates(Some(&caps(&[1, 2, 2, 2])), true, 1, 2)[0], (1, 2));
        assert_eq!(candidates(Some(&caps(&[0, 2])), false, 0, 2)[0], (0, 2));
        // Wide buses.
        assert_eq!(candidates(Some(&caps(&[-1, -1])), true, 6, 6)[0], (6, 6));
    }

    #[test]
    fn a_mono_only_unit_on_a_stereo_slot() {
        let list = candidates(Some(&caps(&[1, 1])), true, 2, 2);
        // The request is tried (capabilities can be wrong), then what the unit states.
        assert_eq!(list[0], (2, 2));
        assert_eq!(list[1], (1, 1));
    }

    #[test]
    fn a_mono_to_stereo_unit_and_a_stereo_only_unit() {
        let list = candidates(Some(&caps(&[1, 2])), true, 2, 2);
        assert_eq!(&list[..2], [(2, 2), (1, 2)]);
        let list = candidates(Some(&caps(&[2, 2])), true, 1, 1);
        assert_eq!(&list[..2], [(1, 1), (2, 2)]);
        // Mono in, stereo out requested, unit supports only equal counts: keep the
        // output width.
        let list = candidates(Some(&caps(&[-1, -1])), true, 1, 2);
        assert_eq!(&list[..2], [(1, 2), (2, 2)]);
    }

    #[test]
    fn sources_have_no_inputs() {
        let list = candidates(Some(&caps(&[0, 2, 0, 1])), false, 0, 2);
        assert_eq!(&list[..2], [(0, 2), (0, 1)]);
        assert!(list.iter().all(|&(inputs, _)| inputs == 0));
        // An instrument in a mono slot that only renders stereo.
        let list = candidates(Some(&caps(&[0, 2])), false, 0, 1);
        assert_eq!(&list[..2], [(0, 1), (0, 2)]);
        // A host that allots an input bus to an instrument does not change that.
        assert!(candidates(None, false, 2, 2).iter().all(|&(inputs, _)| inputs == 0));
    }

    #[test]
    fn candidate_lists_are_sane() {
        let all_caps: [&[i32]; 8] =
            [&[], &[-1, -1], &[-1, -2], &[1, 1], &[2, 2, 1, 1], &[0, 2], &[-1, 2], &[6, 6, 2, 6, -8, 2]];
        for flat in all_caps {
            let caps = caps(flat);
            for has_input in [false, true] {
                for want_in in [0u32, 1, 2, 6, 500] {
                    for want_out in [0u32, 1, 2, 6, 500] {
                        for stated in [None, Some(caps.as_slice())] {
                            let list = candidates(stated, has_input, want_in, want_out);
                            assert!(!list.is_empty());
                            let mut unique = list.clone();
                            unique.sort_unstable();
                            unique.dedup();
                            assert_eq!(unique.len(), list.len(), "{list:?}");
                            for &(inputs, outputs) in &list {
                                assert!((1..=MAX_BUS_CHANNELS).contains(&outputs));
                                assert!(inputs <= MAX_BUS_CHANNELS);
                                assert_eq!(inputs == 0, !has_input, "{list:?}");
                            }
                            // Stereo and mono are always among them.
                            let i = |n: u32| if has_input { n } else { 0 };
                            assert!(list.contains(&(i(2), 2)) && list.contains(&(i(1), 1)));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sidechain_widths() {
        assert_eq!(sidechain_candidates(2), [2, 1]);
        assert_eq!(sidechain_candidates(1), [1, 2]);
        assert_eq!(sidechain_candidates(6), [6, 2, 1]);
        assert_eq!(sidechain_candidates(0), [1, 2]);
    }

    #[test]
    fn equal_widths_pass_straight_through() {
        let map = ChannelMap::new(2, 2);
        assert_eq!(map.input_source(0), Source::Channel(0));
        assert_eq!(map.input_source(1), Source::Channel(1));
        assert_eq!(map.output_source(0), Source::Channel(0));
        assert_eq!(map.output_source(1), Source::Channel(1));
        assert!(map.input_is_direct(0) && map.input_is_direct(1));
        assert!(map.output_is_direct(0) && map.output_is_direct(1));
        assert_eq!(map.used_host_channels(), 2);
    }

    #[test]
    fn mono_unit_on_a_stereo_bus() {
        let map = ChannelMap::new(2, 1);
        assert_eq!(map.input_source(0), Source::Average);
        assert!(!map.input_is_direct(0));
        assert_eq!(map.output_source(0), Source::Channel(0));
        assert_eq!(map.output_source(1), Source::Channel(0));
        assert!(map.output_is_direct(0));
        assert_eq!(map.used_host_channels(), 1);
    }

    #[test]
    fn stereo_unit_on_a_mono_bus() {
        let map = ChannelMap::new(1, 2);
        assert_eq!(map.input_source(0), Source::Channel(0));
        assert_eq!(map.input_source(1), Source::Channel(0));
        assert!(map.input_is_direct(0) && !map.input_is_direct(1));
        assert_eq!(map.output_source(0), Source::Average);
        assert!(!map.output_is_direct(0) && !map.output_is_direct(1));
        assert_eq!(map.used_host_channels(), 1);
    }

    #[test]
    fn other_mismatches_and_missing_buses() {
        // Stereo unit on a six-channel bus: the first two channels.
        let map = ChannelMap::new(6, 2);
        assert_eq!(map.input_source(1), Source::Channel(1));
        assert_eq!(map.output_source(1), Source::Channel(1));
        assert_eq!(map.output_source(2), Source::Silence);
        assert_eq!(map.used_host_channels(), 2);
        // Six-channel unit on a stereo bus: surplus inputs are silent, surplus outputs dropped.
        let map = ChannelMap::new(2, 6);
        assert_eq!(map.input_source(1), Source::Channel(1));
        assert_eq!(map.input_source(2), Source::Silence);
        assert_eq!(map.output_source(1), Source::Channel(1));
        assert!(map.output_is_direct(1) && !map.output_is_direct(2));
        // The host has no such bus: the unit hears silence, its output goes nowhere.
        let map = ChannelMap::new(0, 2);
        assert_eq!(map.input_source(0), Source::Silence);
        assert_eq!(map.output_source(0), Source::Silence);
        assert!(!map.input_is_direct(0) && !map.output_is_direct(0));
        // The unit has no such bus: the host's channels stay silent.
        let map = ChannelMap::new(2, 0);
        assert_eq!(map.output_source(0), Source::Silence);
        assert_eq!(map.input_source(0), Source::Silence);
        assert_eq!(map.used_host_channels(), 0);
        // Out-of-range channels never index anything.
        let map = ChannelMap::new(2, 2);
        assert_eq!(map.input_source(5), Source::Silence);
        assert_eq!(map.output_source(5), Source::Silence);
    }

    #[test]
    fn kernels() {
        let left = [1.0f32, 2.0, 3.0, 4.0];
        let right = [3.0f32, 2.0, 1.0, 0.0];
        let mut out = [9.0f32; 4];
        average(&mut out, &[&left, &right]);
        assert_eq!(out, [2.0, 2.0, 2.0, 2.0]);
        average(&mut out, &[&left]);
        assert_eq!(out, left);
        average(&mut out, &[]);
        assert_eq!(out, [0.0; 4]);
        let mut acc = [1.0f32, 1.0];
        accumulate(&mut acc, &[0.5, 0.25, 100.0]);
        assert_eq!(acc, [1.5, 1.25]);
        scale(&mut acc, 2.0);
        assert_eq!(acc, [3.0, 2.5]);
        // A full round trip of the mono fold: stereo in, mono unit, stereo out.
        let map = ChannelMap::new(2, 1);
        let mut unit_in = [0.0f32; 4];
        assert_eq!(map.input_source(0), Source::Average);
        average(&mut unit_in, &[&left, &right]);
        let unit_out: Vec<f32> = unit_in.iter().map(|s| s * 0.5).collect();
        let host_out: Vec<Vec<f32>> = (0..2)
            .map(|channel| match map.output_source(channel) {
                Source::Channel(c) => {
                    assert_eq!(c, 0);
                    unit_out.clone()
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(host_out, [vec![1.0; 4], vec![1.0; 4]]);
    }
}
