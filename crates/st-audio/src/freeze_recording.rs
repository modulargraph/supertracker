//! Freeze by recording: the prints of a track whose sound is made outside the computer.
//!
//! A live line input (the return of a hardware synth) or an external MIDI instrument cannot be
//! rendered offline. Its prints are recorded instead: the live engine plays the song once, in
//! real time, and the track's strip is captured through a tap after its inserts and before its
//! fader ([`crate::graph::tap`]), which is the point an offline print is taken at.
//!
//! ```text
//!  render thread                         this module, on the command's worker thread
//!  -------------                         ----------------------------------------------
//!  strip, before its fader  ~~ tap ~~>   every block with its graph time, its song frame
//!                                        and the strip's delay  ->  the print's WAV file
//! ```
//!
//! # Where a frame belongs
//!
//! Each block of the tap says at which graph frame it starts, which song frame the engine
//! rendered there (while the transport plays), and by how many frames the strip's signal lags
//! the engine at the tap: the plan's delay compensation, which holds the latency of the
//! strip's inserts and of its live input (the hardware return's round trip,
//! `crate::engine::lines`). The first playing block fixes `origin`, the graph frame at which
//! the engine rendered song frame 0; from then on
//!
//! ```text
//!   song frame of the strip's frame at graph time t  =  t - delay - origin
//! ```
//!
//! and frame `n` of the print is song frame `n`, exactly as in an offline print. The mapping
//! is checked against every later playing block: a seek, a loop or a restart moves it, and the
//! take fails rather than print audio in the wrong place.
//!
//! # A take
//!
//! [`recorded_prints`] publishes a project of its own to the live engine for the length of the
//! takes: the published project without the song loop and the metronome, with the track's
//! input heard in its strip (and the master silent when the mix is not to be heard), and for
//! a pattern's prints with that pattern as the song. It plays from the song's start and writes
//! what the tap delivers until the musical end plus the tail: a fixed tail, or in auto mode
//! until the strip has stayed below -66 dBFS for half a second or the maximum tail is reached
//! (the print is then cut 20 ms after the last louder frame). A pattern's loop print is a take
//! of several passes of which the last one and its tail are kept. Between two takes the strip
//! is given up to two seconds to fall silent, so that what the hardware still rings from one
//! take does not open the next.
//!
//! A take fails, and nothing is kept, when playback does not start, stops before the musical
//! end (Stop cancels the freeze), jumps, when the strip delivers no blocks (the device
//! stopped), when the tap lost audio because this thread could not keep up, or when somebody
//! else publishes a project meanwhile. Whatever happens, the transport is stopped, the tap
//! removed and the project that was published before the takes published again (unless a
//! newer one has taken its place).
//!
//! Not verified on hardware: the return's place in the print is as exact as the latency the
//! devices report.

use crate::engine::{AudioEngine, PlayOptions};
use crate::export::RenderContext;
use crate::freeze::{FreezeRequest, Print, PrintPlan, Prints, CANCELLED};
use crate::graph::tap::{StripTap, TapBlock, NOT_PLAYING};
use serde_json::{json, Value};
use st_project::model::Project;
use st_samples::wav::{Dither, WavEncoding, WavSpec, WavWriter};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Below this a recorded strip counts as silent (hardware has a noise floor).
const SILENCE_DB: f64 = -66.0;
/// Auto tail: how long the strip must stay silent for the take to end, seconds.
const HOLD_SECONDS: f64 = 0.5;
/// Auto tail: what is kept after the last frame above the threshold, seconds.
const KEEP_SECONDS: f64 = 0.02;
/// How long playback may take to start, and how long the tap may deliver nothing.
const PATIENCE: Duration = Duration::from_secs(4);
/// The most a strip is waited for to fall silent between two takes.
const SETTLE: Duration = Duration::from_secs(2);

/// A print being written: song frames in order. The first `skip` frames are left out (the
/// earlier passes of a loop print); what follows the musical end is held back until
/// [`PrintFile::finish`] says how much of it is kept.
struct PrintFile {
    writer: WavWriter<std::io::BufWriter<std::fs::File>>,
    skip: u64,
    musical: u64,
    /// The next song frame.
    placed: u64,
    tail: (Vec<f32>, Vec<f32>),
    peak: f32,
}

impl PrintFile {
    fn create(path: &Path, sample_rate: u32, skip: u64, musical: u64) -> Result<PrintFile, String> {
        let spec = WavSpec { sample_rate, channels: 2, encoding: WavEncoding::Float32, dither: Dither::None };
        let writer = WavWriter::create(path, spec, &[]).map_err(|error| error.to_string())?;
        Ok(PrintFile { writer, skip, musical, placed: 0, tail: (Vec::new(), Vec::new()), peak: 0.0 })
    }

    /// The next `left.len()` song frames.
    fn append(&mut self, left: &[f32], right: &[f32]) -> Result<(), String> {
        let frames = left.len().min(right.len());
        let end = self.placed + frames as u64;
        // What lies before the musical end goes to the file, from frame `skip` on.
        let body = (self.musical.min(end).saturating_sub(self.placed)) as usize;
        let from = (self.skip.saturating_sub(self.placed) as usize).min(body);
        if from < body {
            self.writer.write_planar(&[&left[from..body], &right[from..body]]).map_err(|error| error.to_string())?;
        }
        self.tail.0.extend_from_slice(&left[body..frames]);
        self.tail.1.extend_from_slice(&right[body..frames]);
        if end > self.skip {
            let heard = (self.skip.saturating_sub(self.placed) as usize).min(frames);
            self.peak = left[heard..frames].iter().chain(&right[heard..frames]).fold(self.peak, |peak, sample| peak.max(sample.abs()));
        }
        self.placed = end;
        Ok(())
    }

    /// Completes the file with song frames up to `keep` and returns the frames it holds.
    fn finish(mut self, keep: u64) -> Result<u64, String> {
        let tail = (keep.saturating_sub(self.musical) as usize).min(self.tail.0.len());
        if tail > 0 {
            self.writer.write_planar(&[&self.tail.0[..tail], &self.tail.1[..tail]]).map_err(|error| error.to_string())?;
        }
        let (out, frames) = self.writer.finalize().map_err(|error| error.to_string())?;
        out.into_inner().map_err(|error| format!("The print could not be finished: {}", error.error()))?;
        Ok(frames)
    }
}

/// One take.
struct Take<'a> {
    /// The project the live engine plays for the take.
    project: &'a Value,
    /// The pattern playback starts in.
    pattern: i32,
    /// A fixed tail in frames, or `None` for an automatic one.
    fixed_tail: Option<u64>,
    /// Auto tail: the most that is kept, in frames.
    maximum_tail: u64,
    /// Keep only the last so many musical frames and the tail (a loop print's last pass).
    keep_last: Option<u64>,
    destination: &'a Path,
}

/// The live engine for the length of the takes. Dropping it puts everything back.
struct Session<'a> {
    live: &'a AudioEngine,
    context: &'a RenderContext<'a>,
    tap: Arc<StripTap>,
    original: Project,
    /// The publication that put the take project in effect, once there is one.
    published: Option<u64>,
    /// Takes begun so far.
    takes: usize,
    rate: f64,
    threshold: f32,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.live.stop();
        if let Some(published) = self.published {
            let _ = self.live.publish_unless_changed(&self.original, published);
        }
        let _ = self.live.set_strip_tap(None);
    }
}

impl Session<'_> {
    fn check(&self) -> Result<(), String> {
        if self.context.cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        if self.published.is_some_and(|published| self.live.publication() != published) {
            return Err("The project changed while the freeze was recording. Nothing was changed.".to_string());
        }
        Ok(())
    }

    /// Waits until the strip has been silent for a quarter of a second, for [`SETTLE`] at
    /// most: what the hardware still rings from the take before must not open the next one.
    fn settle(&self) -> Result<(), String> {
        self.tap.clear();
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (began, mut heard, mut quiet) = (Instant::now(), Instant::now(), 0u64);
        loop {
            self.check()?;
            while let Some(block) = self.tap.pop(&mut left, &mut right) {
                let loud = left.iter().chain(&right).any(|sample| sample.abs() > self.threshold);
                quiet = if loud { 0 } else { quiet + block.frames as u64 };
                left.clear();
                right.clear();
                heard = Instant::now();
            }
            if quiet as f64 >= 0.25 * self.rate || began.elapsed() >= SETTLE {
                return Ok(());
            }
            if heard.elapsed() >= PATIENCE {
                return Err("The track's strip delivers no audio: the audio device is not rendering. Nothing was recorded.".to_string());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Records one take into `take.destination`. Returns the print and its peak.
    fn record(&mut self, take: &Take<'_>, progress: &dyn Fn(f64)) -> Result<(Print, f32), String> {
        let live = self.live;
        self.check()?;
        let project = Project::from_value(take.project.clone()).map_err(|error| error.to_string())?;
        live.publish(&project)?;
        self.published = Some(live.publication());
        let duration = live.duration(true);
        let musical = (duration * self.rate) as u64;
        let tail_limit = take.fixed_tail.unwrap_or(take.maximum_tail);
        if !(duration > 0.0) || musical == 0 {
            return Err("The song is empty: there is nothing to record.".to_string());
        }
        if musical + tail_limit > st_project::validate::MAX_FREEZE_FRAMES as u64 {
            return Err(format!("The song is too long to freeze: a print holds {} minutes at most.", (st_project::validate::MAX_FREEZE_FRAMES as f64 / self.rate / 60.0).floor()));
        }
        // The render thread takes the take's mixer with its next block.
        let waiting = Instant::now();
        while !live.plan_adopted() {
            if waiting.elapsed() >= PATIENCE {
                return Err("The audio device is not rendering. Nothing was recorded.".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.takes > 0 {
            self.settle()?;
        }
        self.takes += 1;
        self.tap.clear();
        let dropped_before = self.tap.dropped();
        live.seek(Some(take.pattern), 0, 0);
        live.play(&PlayOptions { song: true, from_start: false, pattern_loop: None, count_in: 0 });

        let skip = take.keep_last.map_or(0, |pass| musical.saturating_sub(pass));
        let mut file = PrintFile::create(take.destination, self.rate.round() as u32, skip, musical)?;
        let limit = musical + tail_limit;
        let hold = (HOLD_SECONDS * self.rate) as u64;
        // The graph frame at which the engine rendered song frame 0, and the strip's delay.
        let mut origin: Option<i64> = None;
        let mut delay: Option<u32> = None;
        // The song frame after the last one above the threshold.
        let mut loud_until = 0u64;
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (began, mut heard) = (Instant::now(), Instant::now());
        let allowed = Duration::from_secs_f64(limit as f64 / self.rate) + Duration::from_secs(15);
        let keep = loop {
            self.check()?;
            while let Some(block) = self.tap.pop(&mut left, &mut right) {
                heard = Instant::now();
                if let Err(error) = self.place(&block, &left, &right, musical, limit, &mut origin, &mut delay, &mut file, &mut loud_until) {
                    // Stop cancels the freeze, and a publication stops playback too: the reason
                    // for a transport that stopped or jumped may follow it by a moment.
                    std::thread::sleep(Duration::from_millis(50));
                    self.check()?;
                    return Err(error);
                }
                left.clear();
                right.clear();
            }
            if self.tap.dropped() != dropped_before {
                return Err("The recording lost audio because the computer could not keep up. Nothing was changed; try again.".to_string());
            }
            let placed = file.placed;
            // An automatic tail is cut shortly after the last sound.
            let cut = (loud_until + (KEEP_SECONDS * self.rate) as u64).clamp(musical, placed.clamp(musical, limit));
            match take.fixed_tail {
                Some(_) if placed >= limit => break limit,
                None if placed >= limit => break cut,
                // Silent for the hold after the musical end: nothing more is to come.
                None if placed >= musical + hold && placed - loud_until.max(musical) >= hold => break cut,
                _ => {}
            }
            if origin.is_none() && began.elapsed() >= PATIENCE {
                return Err("Playback did not start on the track's strip. Nothing was recorded.".to_string());
            }
            if heard.elapsed() >= PATIENCE {
                return Err("The track's strip stopped delivering audio (the audio device stopped). Nothing was changed.".to_string());
            }
            if began.elapsed() >= allowed {
                return Err("The recording did not keep up with playback. Check the audio device and try again.".to_string());
            }
            progress(placed as f64 / limit as f64);
            std::thread::sleep(Duration::from_millis(10));
        };
        live.stop();
        let peak = file.peak;
        let frames = file.finish(keep)?;
        let sha256 = st_project::media::hash_file(take.destination, u64::MAX, None).map_err(|error| error.to_string())?;
        progress(1.0);
        Ok((Print { path: take.destination.to_string_lossy().into_owned(), sha256, frames, musical: musical - skip }, peak))
    }

    /// Puts one block of the tap where it belongs in the print.
    #[allow(clippy::too_many_arguments)]
    fn place(&self, block: &TapBlock, left: &[f32], right: &[f32], musical: u64, limit: u64, origin: &mut Option<i64>, delay: &mut Option<u32>, file: &mut PrintFile, loud_until: &mut u64) -> Result<(), String> {
        let time = block.time as i64;
        if block.song_frame != NOT_PLAYING {
            let rendered = time - block.song_frame;
            match *origin {
                None => *origin = Some(rendered),
                // The engine's clock is exact; a frame of slack covers a fractional position.
                Some(known) if (known - rendered).abs() > 1 && time - known < musical as i64 => {
                    return Err("Playback jumped while the freeze was recording (a seek, a loop or a restart). Nothing was changed; try again without touching the transport.".to_string());
                }
                Some(_) => {}
            }
        } else if let Some(known) = *origin {
            if time - known < musical as i64 - 2 {
                return Err("Playback stopped before the end of the song. Nothing was changed.".to_string());
            }
        }
        // Blocks before playback started hold nothing of the song.
        let Some(known) = *origin else { return Ok(()) };
        if *delay.get_or_insert(block.arrival) != block.arrival {
            return Err("The mixer changed while the freeze was recording. Nothing was changed.".to_string());
        }
        // The song frame of the block's first frame.
        let first = time - block.arrival as i64 - known;
        let frames = left.len() as i64;
        if first + frames <= file.placed as i64 || file.placed >= limit {
            return Ok(());
        }
        // A hole (a block the tap could not take) is silence at its place; the take fails for
        // it once the tap's count says so.
        while (file.placed as i64) < first && file.placed < limit {
            const ZEROS: [f32; 1024] = [0.0; 1024];
            let count = ((first - file.placed as i64) as u64).min(limit - file.placed).min(1024) as usize;
            file.append(&ZEROS[..count], &ZEROS[..count])?;
        }
        let from = (file.placed as i64 - first).max(0) as usize;
        let count = ((frames as usize).saturating_sub(from)).min((limit - file.placed) as usize);
        let (left, right) = (&left[from..from + count], &right[from..from + count]);
        if let Some(last) = left.iter().zip(right).rposition(|(l, r)| l.abs() > self.threshold || r.abs() > self.threshold) {
            *loud_until = file.placed + last as u64 + 1;
        }
        file.append(left, right)
    }
}

/// The song and the patterns of `plan`, recorded from the live engine. See the module
/// documentation. Files it creates are added to `written`.
pub(crate) fn recorded_prints(context: &RenderContext<'_>, plan: &PrintPlan<'_>, request: &FreezeRequest, written: &mut Vec<String>) -> Result<Prints, String> {
    let live = context.live;
    if !live.is_live() {
        return Err("Freeze by recording plays the song in real time and needs a running audio device.".to_string());
    }
    if live.transport_playing() {
        return Err("Stop playback before freezing by recording.".to_string());
    }
    if live.live().is_recording() {
        return Err("Finish the audio recording before freezing by recording.".to_string());
    }
    let original = live.project().ok_or("There is no project to freeze yet.")?;
    let (inputs, _) = live.line_inputs();
    if let Some(row) = inputs.iter().find(|row| row["track"] == json!(plan.index) && row["state"] == "paused") {
        return Err(format!("The live input of {} is not running: {}", plan.name, row["status"].as_str().unwrap_or("check the audio input.")));
    }
    let rate = live.sample_rate();
    // What the live engine plays for the takes.
    let mut base = original.to_value();
    base["loopSong"] = json!(false);
    base["metronome"] = json!(false);
    if let Some(input) = base["tracks"][plan.index]["audioInput"].as_object_mut() {
        // The print needs the input in the strip, whether or not it is monitored otherwise.
        input.remove("monitor");
    }
    if !request.monitor {
        base["masterVolume"] = json!(0.0);
    }
    let tap = Arc::new(StripTap::new((rate * 4.0) as usize));
    live.set_strip_tap(Some((plan.index, tap.clone())))?;
    let mut session = Session { live, context, tap, original, published: None, takes: 0, rate, threshold: 10f64.powf(SILENCE_DB / 20.0) as f32 };

    let maximum_tail = (plan.maximum_tail * rate) as u64;
    let fixed_tail = plan.tail.map(|tail| (tail * rate) as u64);
    let used: &[i64] = if request.patterns { &plan.used } else { &[] };
    let steps = 1 + 2 * used.len();
    let song_path = plan.file("song");
    written.push(song_path.to_string_lossy().into_owned());
    let start = base["activePattern"].as_f64().filter(|number| number.is_finite()).map_or(0, |number| number as i32);
    let (song, peak) = session.record(&Take { project: &base, pattern: start, fixed_tail, maximum_tail, keep_last: None, destination: &song_path }, &|fraction| (context.progress)(fraction / steps as f64, "song"))?;
    let mut notes = Vec::new();
    if peak <= session.threshold {
        // A print without any sound is worth a word: nothing came back from the hardware.
        notes.push(format!("The recording of {} is silent: check the hardware's return and the track's live input.", plan.name));
    }

    let mut patterns = Vec::new();
    for (position, number) in used.iter().enumerate() {
        let step = |part: usize| move |fraction: f64| (context.progress)((1 + 2 * position + part) as f64 / steps as f64 + fraction / steps as f64, "patterns");
        let mut project = base.clone();
        project["audioClips"] = json!([]);
        project["sequence"] = json!([number]);
        let head_path = plan.file(&format!("p{number}-head"));
        written.push(head_path.to_string_lossy().into_owned());
        let (head, _) = session.record(&Take { project: &project, pattern: *number as i32, fixed_tail, maximum_tail, keep_last: None, destination: &head_path }, &step(0))?;
        let pass = head.musical;
        let tail = head.frames.saturating_sub(pass);
        let passes = plan.loop_passes(tail, pass, rate);
        project["sequence"] = json!(vec![*number; passes as usize]);
        let loop_path = plan.file(&format!("p{number}-loop"));
        written.push(loop_path.to_string_lossy().into_owned());
        let (looped, _) = session.record(&Take { project: &project, pattern: *number as i32, fixed_tail: Some(tail), maximum_tail, keep_last: Some(pass), destination: &loop_path }, &step(1))?;
        patterns.push(json!({
            "pattern": number, "path": head.path, "sha256": head.sha256, "frames": head.frames, "passFrames": pass, "passes": passes,
            "loop": { "path": looped.path, "sha256": looped.sha256, "frames": looped.frames },
        }));
    }
    drop(session);
    let unprinted: Vec<i64> = if request.patterns { Vec::new() } else { plan.used.clone() };
    Ok(Prints { rate, song, patterns, unprinted, notes })
}
