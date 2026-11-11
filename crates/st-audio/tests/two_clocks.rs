//! An input device with a clock of its own, in simulated time.
//!
//! No test here waits for real time: `Clocks` is two devices, each with its own crystal, and
//! decides when every callback of either happens. The input is the manual one with a clock of
//! its own (`null:manual:<rate>:<block>`), which takes the time of each delivery from the test
//! ([`InputHub::deliver_at`]); the render side is told the time of each of its callbacks
//! ([`LineFeed::read`] directly, or `Renderer::render_at` through the engine). Simulated time
//! is the output's: the output device is exactly at its nominal rate, the input device is off
//! by so many parts per million, and both callbacks come up to half a millisecond late.
//!
//! The input captures two steady tones, as an analogue source would give them: 1 kHz on
//! channel 0 and 20 Hz on channel 1, at half of full scale. On the output's clock the first
//! must still be 1 kHz (its frequency per second-long window says how much the converter's
//! ratio wanders) and the second says, by its phase, how late the input is to the frame.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::inputs::{InputHub, InputRequest, Inputs, LineFeed, LineRoute, LineState};
use st_audio::live::{RecordRequest, RecordSource};
use st_audio::{AudioEngine, EngineOptions};
use std::f64::consts::TAU;
use std::sync::Arc;

const OUTPUT_RATE: f64 = 48_000.0;
/// Simulated time of the first output callback, in seconds (a whole number of cycles of both
/// tones, so their phase at an output frame is the delay alone).
const START: f64 = 1.0;
const TONE: f64 = 1_000.0;
const SLOW: f64 = 20.0;

/// What the input device captures at `seconds` of simulated time.
fn captured(seconds: f64, channel: usize) -> f32 {
    (0.5 * (TAU * if channel == 0 { TONE } else { SLOW } * seconds).sin()) as f32
}

/// Two devices and when their callbacks happen.
struct Clocks {
    /// The input device's real rate in frames per second of simulated time, and its block.
    input_rate: f64,
    input_block: usize,
    output_block: usize,
    /// Callbacks come up to this many seconds late.
    jitter: f64,
    noise: u64,
    /// How late the next input block is.
    late: f64,
    /// Input blocks delivered and output blocks begun so far.
    delivered: u64,
    rendered: u64,
    /// Seconds of audio the input device captured and never delivered.
    lost: f64,
    /// Input blocks that arrive before this time are delivered at it, all at once.
    held_until: f64,
}

impl Clocks {
    fn new(nominal: f64, ppm: f64, input_block: usize, output_block: usize) -> Clocks {
        let mut clocks = Clocks { input_rate: nominal * (1.0 + ppm * 1e-6), input_block, output_block, jitter: 0.0005, noise: 0x9e37_79b9_7f4a_7c15, late: 0.0, delivered: 0, rendered: 0, lost: 0.0, held_until: 0.0 };
        clocks.late = clocks.lateness();
        // The input was switched on shortly before the output's first callback.
        clocks.delivered = ((START - 0.05) * clocks.input_rate / input_block as f64) as u64;
        clocks
    }

    fn lateness(&mut self) -> f64 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 7;
        self.noise ^= self.noise << 17;
        (self.noise >> 11) as f64 / (1u64 << 53) as f64 * self.jitter
    }

    /// The nominal time of the next output callback.
    fn now(&self) -> f64 {
        START + (self.rendered * self.output_block as u64) as f64 / OUTPUT_RATE
    }

    /// The next output callback: delivers the input blocks that have arrived by then and
    /// returns the callback's time in nanoseconds.
    fn next(&mut self, hub: &InputHub) -> u64 {
        let time = self.now() + self.lateness();
        loop {
            let (first, block) = (self.delivered * self.input_block as u64, self.input_block);
            // A block arrives when its last frame has been captured.
            let arrival = ((first + block as u64) as f64 / self.input_rate + self.lost + self.late).max(self.held_until);
            if arrival > time {
                break;
            }
            let (rate, lost) = (self.input_rate, self.lost);
            hub.deliver_at((arrival * 1e9) as u64, block, 8, |frame, channel| captured((first + frame as u64) as f64 / rate + lost, channel));
            self.delivered += 1;
            self.late = self.lateness();
        }
        self.rendered += 1;
        (time * 1e9) as u64
    }
}

/// One second of the output, measured.
#[derive(Clone, Copy, Default)]
struct Second {
    /// Amplitude of the left side (from its power) and phase of the 1 kHz tone on it.
    level: f64,
    phase: f64,
    /// Frames the 20 Hz tone on the right is late (modulo its period of 2 400 frames).
    delay: f64,
}

/// Measures the two tones second by second.
#[derive(Default)]
struct Analysis {
    frame: u64,
    sums: [f64; 5],
    seconds: Vec<Second>,
}

impl Analysis {
    fn add(&mut self, left: &[f32], right: &[f32]) {
        for (left, right) in left.iter().zip(right) {
            let seconds = self.frame as f64 / OUTPUT_RATE;
            let (tone, slow) = ((TAU * TONE * seconds).sin_cos(), (TAU * SLOW * seconds).sin_cos());
            self.sums[0] += *left as f64 * tone.0;
            self.sums[1] += *left as f64 * tone.1;
            self.sums[2] += *right as f64 * slow.0;
            self.sums[3] += *right as f64 * slow.1;
            self.sums[4] += *left as f64 * *left as f64;
            self.frame += 1;
            if self.frame.is_multiple_of(OUTPUT_RATE as u64) {
                let [i, q, slow_i, slow_q, squares] = std::mem::take(&mut self.sums);
                // sin(w t - w d) has the phase -w d: the delay in frames.
                let delay = (-slow_q.atan2(slow_i) / TAU / SLOW * OUTPUT_RATE).rem_euclid(OUTPUT_RATE / SLOW);
                self.seconds.push(Second { level: (2.0 * squares / OUTPUT_RATE).sqrt(), phase: q.atan2(i), delay });
            }
        }
    }

    /// The frequency of the tone in second `index`, from its phase against the second before.
    fn hertz(&self, index: usize) -> f64 {
        let turn = (self.seconds[index].phase - self.seconds[index - 1].phase + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
        TONE + turn / TAU
    }

    /// How far the delay of second `index` is from `latency` frames.
    fn off(&self, index: usize, latency: u32) -> f64 {
        let period = OUTPUT_RATE / SLOW;
        (self.seconds[index].delay - latency as f64 + period / 2.0).rem_euclid(period) - period / 2.0
    }

    /// The largest deviations in the seconds `range`: of the tone's frequency in Hz, of its
    /// level (relative), and of the delay from `latency` in frames.
    fn worst(&self, range: std::ops::Range<usize>, latency: u32) -> (f64, f64, f64) {
        range.fold((0.0f64, 0.0f64, 0.0f64), |worst, index| (worst.0.max((self.hertz(index) - TONE).abs()), worst.1.max((self.seconds[index].level / 0.5 - 1.0).abs()), worst.2.max(self.off(index, latency).abs())))
    }
}

/// The input `uid` as the one stereo line input of an engine-less [`Inputs`] at 48 kHz.
fn feed(uid: &str, output_block: u32) -> (Inputs, Arc<LineFeed>) {
    let inputs = Inputs::default();
    inputs.set_format(OUTPUT_RATE, output_block, "Speakers");
    inputs.configure(&InputRequest { uid: Some(uid.to_string()), ..InputRequest::default() }).unwrap();
    let states = inputs.set_line_routes(&[LineRoute { track: 0, device_uid: uid.to_string(), channels: [0, 1] }], false);
    let LineState::Live { feed, input_latency: 0 } = &states[0] else { panic!("not live: {states:?}") };
    (inputs, feed.clone())
}

/// Ten minutes of an input at `nominal` Hz whose clock is `ppm` off, read by a 48 kHz output.
fn ten_minutes(nominal: u32, ppm: f64, input_block: usize, output_block: usize) {
    let (inputs, feed) = feed(&format!("null:manual:{nominal}:{input_block}"), output_block as u32);
    let mut clocks = Clocks::new(nominal as f64, ppm, input_block, output_block);
    let mut analysis = Analysis::default();
    let (mut left, mut right) = (vec![0.0f32; output_block], vec![0.0f32; output_block]);
    let (mut least, mut most) = (usize::MAX, 0usize);
    let seconds = 600;
    while analysis.seconds.len() < seconds {
        let time = clocks.next(inputs.hub());
        // The feed measures for a tenth of a second before it starts, in silence.
        if !feed.read(time, &mut left, &mut right) {
            assert!(analysis.frame < 4_800 + output_block as u64, "the feed is heard from its first tenth of a second on");
            left.fill(0.0);
            right.fill(0.0);
        }
        analysis.add(&left, &right);
        if analysis.frame > 48_000 {
            least = least.min(feed.waiting());
            most = most.max(feed.waiting());
        }
    }
    let latency = feed.latency();
    let (start, settled) = (analysis.worst(2..60, latency), analysis.worst(60..seconds, latency));
    // The mean frequency of the last nine minutes, from the phase at their ends.
    let turns: f64 = (61..seconds).map(|index| analysis.hertz(index) - TONE).sum();
    let mean = TONE + turns / (seconds - 61) as f64;
    println!(
        "{nominal} Hz {ppm:+} ppm, blocks {input_block}/{output_block}: latency {latency} frames; measured {:+.2} ppm; first minute: tone within {:.4} Hz, delay within {:.2} frames; after it: tone within {:.5} Hz ({:.2} ppm), mean {mean:.7} Hz, level within {:.5} %, delay within {:.2} frames; {least} to {most} frames waiting; {} underruns, {} overruns",
        feed.drift_ppm().unwrap(),
        start.0,
        start.2,
        settled.0,
        settled.0 / TONE * 1e6,
        settled.1 * 100.0,
        settled.2,
        feed.underruns(),
        feed.overruns()
    );
    // Not one gap and not one skip in ten minutes.
    assert_eq!((feed.underruns(), feed.overruns()), (0, 0));
    // The ring never ran dry and never held more than the delay the feed keeps (the delay is
    // counted to the input's position between blocks, so less than it waits in the ring).
    let target = latency as f64 * nominal as f64 / OUTPUT_RATE;
    assert!(least > 0 && (most as f64) < target + 8.0, "{least} to {most} frames waiting for a delay of {target}");
    // The tone is the tone: while the loop finds the ratio it is off by less than the clocks
    // are (a quarter of a hertz would be 250 ppm, 0.4 cent), afterwards by a few millionths.
    assert!(start.0 < 0.25 && settled.0 < 0.02, "the tone wanders by {} Hz, then {} Hz", start.0, settled.0);
    assert!((mean - TONE).abs() < 2e-4, "{mean} Hz on average");
    assert!(start.1 < 1e-4 && settled.1 < 1e-4, "level off by {} and {}", start.1, settled.1);
    // The input is as late as the feed says: to 10 frames from the second second on, while
    // the loop finds the ratio, and to 4 frames (a twelfth of a millisecond) once it has it.
    assert!(start.2 < 10.0 && settled.2 < 4.0, "delay off by {} and then {} frames", start.2, settled.2);
    assert!((feed.drift_ppm().unwrap() - ppm).abs() < 5.0);
}

#[test]
fn ten_minutes_from_an_input_200_ppm_fast_have_no_gap_no_skip_and_the_right_pitch() {
    ten_minutes(48_000, 200.0, 512, 512);
}

#[test]
fn ten_minutes_from_an_input_200_ppm_slow_have_no_gap_no_skip_and_the_right_pitch() {
    // Other block sizes on both sides: 10 ms of input against 256 frames of output.
    ten_minutes(48_000, -200.0, 480, 256);
}

#[test]
fn ten_minutes_from_a_44_1_khz_input_have_no_gap_no_skip_and_the_right_pitch() {
    ten_minutes(44_100, 75.0, 441, 512);
}

/// A feed that has run for `seconds`, and its measurements.
struct Running {
    inputs: Inputs,
    feed: Arc<LineFeed>,
    clocks: Clocks,
    analysis: Analysis,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Running {
    fn new(ppm: f64) -> Running {
        let (inputs, feed) = feed("null:manual:48000:480", 512);
        let mut running = Running { inputs, feed, clocks: Clocks::new(48_000.0, ppm, 480, 512), analysis: Analysis::default(), left: vec![0.0; 512], right: vec![0.0; 512] };
        running.run(40.0);
        assert_eq!((running.feed.underruns(), running.feed.overruns()), (0, 0));
        running
    }

    /// One output callback. Returns whether the feed was heard.
    fn step(&mut self) -> bool {
        let time = self.clocks.next(self.inputs.hub());
        let heard = self.feed.read(time, &mut self.left, &mut self.right);
        if !heard {
            self.left.fill(0.0);
            self.right.fill(0.0);
        }
        self.analysis.add(&self.left, &self.right);
        heard
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * OUTPUT_RATE / 512.0) as usize {
            self.step();
        }
    }

    /// Runs on for six seconds and returns how far the delay is from the feed's latency in
    /// the last two of them, in frames, with the frequency error of the tone there. (A jump
    /// lands as exactly as the callbacks around it were punctual, which here is to a quarter
    /// of a millisecond; the loop then closes the rest slowly, so that it is not heard.)
    fn recovered(&mut self) -> (f64, f64) {
        self.run(6.0);
        let last = self.analysis.seconds.len();

        let worst = self.analysis.worst(last - 2..last, self.feed.latency());
        (worst.2, worst.0)
    }
}

#[test]
fn a_stall_and_a_burst_are_counted_and_leave_no_offset() {
    // The input stalls for 150 ms and then delivers everything it held back (a USB hiccup):
    // silence where frames were missing, one underrun, and the delay is what it was.
    let mut running = Running::new(120.0);
    let before = running.analysis.worst(38..40, running.feed.latency());
    running.clocks.held_until = running.clocks.now() + 0.150;
    let (delay, tone) = running.recovered();
    println!("stall and burst: delay off by {:.2} frames before, {delay:.2} after ({} underruns, {} overruns), tone within {tone:.4} Hz", before.2, running.feed.underruns(), running.feed.overruns());
    assert_eq!((running.feed.underruns(), running.feed.overruns()), (1, 0));
    assert!(before.2 < 4.0 && delay < 4.0 && tone < 0.02, "{delay} frames, {tone} Hz");

    // The input loses 150 ms (it captured them and never delivered them): the feed waits for
    // the input to catch up, in silence, and is back on its delay. The same incident is
    // counted once.
    running.clocks.lost += 0.150;
    let (delay, tone) = running.recovered();
    println!("lost input: delay off by {delay:.2} frames, tone within {tone:.4} Hz ({} underruns, {} overruns)", running.feed.underruns(), running.feed.overruns());
    assert_eq!((running.feed.underruns(), running.feed.overruns()), (2, 0));
    assert!(delay < 8.0 && tone < 0.05, "{delay} frames, {tone} Hz");

    // The output stands still for 150 ms (its callbacks do not come, and it does not make up
    // for them): what waited too long is dropped, one overrun.
    for _ in 0..14 {
        running.clocks.next(running.inputs.hub());
        running.analysis.add(&[0.0; 512], &[0.0; 512]);
    }
    let (delay, tone) = running.recovered();
    println!("stalled output: delay off by {delay:.2} frames, tone within {tone:.4} Hz ({} underruns, {} overruns)", running.feed.underruns(), running.feed.overruns());
    assert_eq!((running.feed.underruns(), running.feed.overruns()), (2, 1));
    assert!(delay < 8.0 && tone < 0.05, "{delay} frames, {tone} Hz");

    // The input is gone for two seconds (unplugged and back): the feed is silent after half a
    // second, starts over when blocks arrive again, and has kept the ratio it had measured.
    let drift = running.feed.drift_ppm().unwrap();
    running.clocks.lost += 2.0;
    let mut silent = 0;
    for _ in 0..(2.5 * OUTPUT_RATE / 512.0) as usize {
        silent += usize::from(!running.step());
    }
    let (delay, tone) = running.recovered();
    println!("input gone for 2 s: {silent} silent blocks, delay off by {delay:.2} frames, tone within {tone:.4} Hz, measured {drift:+.1} ppm before and {:+.1} after", running.feed.drift_ppm().unwrap());
    assert_eq!((running.feed.underruns(), running.feed.overruns()), (3, 1));
    assert!((140..=160).contains(&silent), "{silent} blocks without the feed");
    assert!(delay < 8.0 && tone < 0.05 && (running.feed.drift_ppm().unwrap() - drift).abs() < 10.0, "{delay} frames, {tone} Hz");

    // Nobody reads for three seconds while the input goes on (the render thread is busy
    // elsewhere): the ring runs full, what is in it is stale and dropped, one overrun.
    for _ in 0..(3.0 * OUTPUT_RATE / 512.0) as usize {
        running.clocks.next(running.inputs.hub());
        running.analysis.add(&[0.0; 512], &[0.0; 512]);
    }
    let (delay, tone) = running.recovered();
    println!("nobody read for 3 s: delay off by {delay:.2} frames, tone within {tone:.4} Hz ({} underruns, {} overruns)", running.feed.underruns(), running.feed.overruns());
    assert_eq!((running.feed.underruns(), running.feed.overruns()), (3, 2));
    assert!(delay < 8.0 && tone < 0.05, "{delay} frames, {tone} Hz");
}

fn row(engine: &AudioEngine, track: usize) -> Value {
    engine.line_inputs().0.into_iter().find(|row| row["track"] == json!(track)).unwrap_or(Value::Null)
}

#[test]
fn a_44_1_khz_input_reaches_its_track_and_a_take_at_the_engines_rate_and_pitch() {
    // The whole way: a 44.1 kHz device 150 ppm slow, routed into track a of an engine at
    // 48 kHz, and its chosen channels recorded. The test is both devices' threads.
    const BLOCK: usize = 256;
    let uid = "null:manual:44100:441";
    let mut options = EngineOptions::offline();
    options.samples = samples();
    options.buffer_size = BLOCK as u32;
    let engine = AudioEngine::new(options).unwrap();
    engine.live().inputs().configure(&InputRequest { uid: Some(uid.to_string()), channels: Some(vec![0, 1]), ..InputRequest::default() }).unwrap();
    // Track a takes the input in; track b plays the tone sample of the test projects.
    let mut project = project(2);
    project["patterns"][0]["steps"][0] = json!(vec![Value::Null; 16]);
    project["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": uid, "channels": [0, 1], "gain": 1 });
    project["tracks"][1]["mute"] = json!(true);
    engine.publish(&parse(&project)).unwrap();
    let line = row(&engine, 0);
    // 256 output frames are 235.2 of the input's, its block is 441, the filter reads 46
    // ahead, the margin is 5 ms (220.5): 943 input frames, 1 026 of the engine's.
    assert_eq!((line["state"].clone(), line["clock"].clone(), line["latencyFrames"].clone()), (json!("live"), json!("converted"), json!(1_026)));
    let mut devices = json!({});
    engine.live().decorate_devices(&mut devices);
    assert_eq!((devices["inputRunning"].clone(), devices["inputSampleRate"].clone()), (json!(true), json!(44_100.0)));

    let renderer = engine.renderer();
    let hub = engine.live().inputs().hub().clone();
    let mut clocks = Clocks::new(44_100.0, -150.0, 441, BLOCK);
    let mut analysis = Analysis::default();
    let (mut left, mut right) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut step = |analysis: &mut Analysis| {
        let time = clocks.next(&hub);
        renderer.lock().unwrap().render_at(time, &mut left, &mut right);
        analysis.add(&left, &right);
    };
    while analysis.seconds.len() < 45 {
        step(&mut analysis);
    }
    assert_eq!(engine.status()["latencyCompensation"]["frames"], 1_026, "the mixer compensates for the feed's delay");
    let began = analysis.frame as i64;
    // A take of the chosen channels, from second 45 on.
    let path = std::env::temp_dir().join(format!("st-audio-two-clocks-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&path);
    engine.live().record(&RecordRequest { source: RecordSource::Input, path: path.clone(), monitor: false, monitor_volume: 0.0, output_latency_frames: 0 }).unwrap();
    while analysis.seconds.len() < 60 {
        step(&mut analysis);
    }
    // Stop arrives from another thread while the output keeps rendering: the take holds at least
    // every frame rendered between Record and this point.
    let asked = analysis.frame as i64;
    let stopper = std::thread::spawn({
        let engine = engine.clone();
        move || engine.live().stop_record().expect("a take was running")
    });
    while !stopper.is_finished() {
        step(&mut analysis);
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    let take = stopper.join().unwrap();

    // On the track: the tone at its pitch and level, as late as the mixer was told.
    let (_, settled) = (analysis.worst(2..30, 1_026), analysis.worst(30..60, 1_026));
    let line = row(&engine, 0);
    println!("44.1 kHz -150 ppm through the engine: tone within {:.4} Hz, level within {:.4} %, delay within {:.2} frames of 1026; {line}", settled.0, settled.1 * 100.0, settled.2);
    assert!(settled.0 < 0.05 && settled.1 < 5e-4 && settled.2 < 3.0, "{settled:?}");
    assert_eq!((line["underruns"].clone(), line["overruns"].clone()), (json!(0), json!(0)));
    assert!((line["driftPpm"].as_f64().unwrap() + 150.0).abs() < 10.0, "{line}");

    // The take: at the engine's rate, one frame per frame the output rendered between Record
    // and Stop (less the feed's delay, which is left out of its start), and at pitch.
    assert_eq!((take.file.problem.clone(), take.file.dropped, take.file.sample_rate, take.file.channels, take.compensation_frames), (None, 0, 48_000, 2, 1_026));
    let (start, end) = (take.file.start_frame.unwrap(), take.file.end_frame.unwrap());
    assert_eq!(end - start, take.file.frames as i64);
    assert_eq!(start, began + 1_026, "the take began with the first block after Record");
    let recorded = st_audio::samples::read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((recorded.frames() as u64, recorded.sample_rate), (take.file.frames, 48_000.0), "the file is what the take says");
    assert!(asked - began > 14 * 48_000 && recorded.frames() as i64 >= asked - began - 1_026, "everything rendered between Record and Stop was recorded: {} of {} frames", recorded.frames(), asked - began);
    // The take's own feed began with the take, from the clock difference the track's feed
    // had measured by then. Frame n of the file is what the input captured at graph frame
    // start + n - 1 026: the delay left out is the delay there was.
    let mut file = Analysis { frame: (start - 1_026) as u64, ..Analysis::default() };
    let skip = 48_000 - (start - 1_026) as usize % 48_000;
    file.frame += skip as u64;
    file.add(&recorded.left[skip..], &recorded.right[skip..]);
    let seconds = file.seconds.len();
    let in_file = file.worst(1..seconds, 0);
    println!("the take: {} frames at {} Hz; from its first whole second on the tone is within {:.4} Hz, the level within {:.4} %, and it is within {:.2} frames of where the output heard it", recorded.frames(), recorded.sample_rate, in_file.0, in_file.1 * 100.0, in_file.2);
    assert!(seconds >= 13 && in_file.0 < 0.05 && in_file.1 < 5e-4 && in_file.2 < 3.0, "{in_file:?}");
    let _ = std::fs::remove_file(path);
}
