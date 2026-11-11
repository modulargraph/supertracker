//! Offline renders of small projects, checked against what the mixer's rules say they must be.
//!
//! Each test renders a reference (one track at unity straight to the master) and a variation,
//! and compares them frame by frame, so nothing here depends on how the engine's sampler shapes
//! the tone.

mod common;

use common::*;
use serde_json::json;

const FRAMES: usize = 24_000;

fn reference() -> (Vec<f32>, Vec<f32>) {
    let (left, right) = render(&project(1), FRAMES);
    assert!(peak(&left) > 0.2 && left == right, "the reference tone is audible and centred");
    (left, right)
}

#[test]
fn a_fader_scales_and_a_pan_is_a_balance() {
    let (reference, _) = reference();
    let mut half = project(1);
    half["tracks"][0]["volume"] = json!(0.5);
    let (left, right) = render(&half, FRAMES);
    assert_signal(&left, |i| reference[i] * 0.5, 1e-7, "volume 0.5");
    assert_eq!(left, right);

    let mut loud = project(1);
    loud["tracks"][0]["volume"] = json!(2.0);
    loud["masterVolume"] = json!(0.25);
    assert_signal(&render(&loud, FRAMES).0, |i| reference[i] * 0.5, 1e-6, "track at 2, master at 0.25");

    // Pan right by 0.5: the left channel loses half, the right one stays at unity.
    let mut panned = project(1);
    panned["tracks"][0]["pan"] = json!(0.5);
    let (left, right) = render(&panned, FRAMES);
    assert_signal(&left, |i| reference[i] * 0.5, 1e-7, "pan 0.5, left");
    assert_signal(&right, |i| reference[i], 0.0, "pan 0.5, right");
    panned["tracks"][0]["pan"] = json!(-1.0);
    let (left, right) = render(&panned, FRAMES);
    assert_signal(&left, |i| reference[i], 0.0, "pan -1, left");
    assert_eq!(peak(&right), 0.0, "hard left leaves nothing on the right");

    let mut muted = project(1);
    muted["tracks"][0]["mute"] = json!(true);
    let (left, right) = render(&muted, FRAMES);
    assert_eq!((peak(&left), peak(&right)), (0.0, 0.0), "a muted track is silent");
}

#[test]
fn a_group_bus_sums_its_tracks_and_a_disconnected_track_is_silent() {
    let (reference, _) = reference();
    let mut grouped = project(3);
    grouped["routing"]["buses"] = json!([bus("g", "group", 0.5)]);
    grouped["routing"]["outputs"] = json!({ "track:a": "bus:g", "track:b": "bus:g", "track:c": null });
    grouped["tracks"][1]["volume"] = json!(0.5);
    // (a + 0.5 b) through a bus at 0.5; c is disconnected.
    assert_signal(&render(&grouped, FRAMES).0, |i| reference[i] * 0.75, 1e-6, "group bus");
    grouped["routing"]["buses"][0]["pan"] = json!(1.0);
    let (left, right) = render(&grouped, FRAMES);
    assert_eq!(peak(&left), 0.0, "a bus pans like a track");
    assert_signal(&right, |i| reference[i] * 0.75, 1e-6, "group bus, panned right");
    grouped["routing"]["buses"][0]["mute"] = json!(true);
    assert_eq!(peak(&render(&grouped, FRAMES).1), 0.0, "a muted bus silences its tracks");
}

#[test]
fn sends_tap_before_or_after_the_fader_with_their_gain() {
    let (reference, _) = reference();
    let send = |tap: &str, gain: f64| json!({ "id": "s", "source": "track:a", "tap": tap, "destination": "bus:r", "gain": gain, "enabled": true });
    let mut sent = project(1);
    sent["routing"]["buses"] = json!([bus("r", "return", 1.0)]);
    sent["routing"]["sends"] = json!([send("postFader", 0.25)]);
    sent["tracks"][0]["volume"] = json!(0.5);
    // Dry 0.5 plus 0.25 of the post-fader 0.5.
    assert_signal(&render(&sent, FRAMES).0, |i| reference[i] * 0.625, 1e-6, "post-fader send");
    // A pre-fader send ignores the fader: with the track's output disconnected only it remains.
    sent["routing"]["sends"] = json!([send("preFader", 0.25)]);
    sent["routing"]["outputs"] = json!({ "track:a": null });
    assert_signal(&render(&sent, FRAMES).0, |i| reference[i] * 0.25, 1e-6, "pre-fader send");
    sent["tracks"][0]["mute"] = json!(true);
    assert_signal(&render(&sent, FRAMES).0, |i| reference[i] * 0.25, 1e-6, "a pre-fader send of a muted track still passes");
    sent["routing"]["sends"] = json!([send("postFader", 0.25)]);
    assert_eq!(peak(&render(&sent, FRAMES).0), 0.0, "a post-fader send of a muted track is silent");
    let mut disabled = send("preFader", 0.25);
    disabled["enabled"] = json!(false);
    sent["routing"]["sends"] = json!([disabled]);
    assert_eq!(peak(&render(&sent, FRAMES).0), 0.0, "a disabled send passes nothing");
}

#[test]
fn solo_keeps_what_feeds_and_what_follows_the_soloed_strip() {
    let (reference, _) = reference();
    let level = |project: &serde_json::Value| -> f32 {
        let (left, _) = render(project, FRAMES);
        // Every case is the reference times a constant: find it at the loudest frame.
        let (at, _) = reference.iter().enumerate().fold((0, 0.0f32), |best, (index, sample)| if sample.abs() > best.1 { (index, sample.abs()) } else { best });
        let gain = left[at] / reference[at];
        assert_signal(&left, |i| reference[i] * gain, 2e-6, "solo case");
        (gain * 1000.0).round() / 1000.0
    };
    // a -> group g (0.5) -> master; b -> master; c -> master, and c sends 0.25 to return r.
    let mut base = project(3);
    base["routing"]["buses"] = json!([bus("g", "group", 0.5), bus("r", "return", 1.0)]);
    base["routing"]["outputs"] = json!({ "track:a": "bus:g" });
    base["routing"]["sends"] = json!([{ "id": "s", "source": "track:c", "tap": "postFader", "destination": "bus:r", "gain": 0.25 }]);
    assert_eq!(level(&base), 0.5 + 1.0 + 1.25, "nothing soloed");

    let solo = |owners: &[&str]| {
        let mut project = base.clone();
        for owner in owners {
            match owner.strip_prefix("track:") {
                Some(name) => project["tracks"][(name.as_bytes()[0] - b'a') as usize]["solo"] = json!(true),
                None => {
                    let index = if *owner == "bus:g" { 0 } else { 1 };
                    project["routing"]["buses"][index]["solo"] = json!(true);
                }
            }
        }
        project
    };
    assert_eq!(level(&solo(&["track:a"])), 0.5, "a soloed track plays through its group");
    assert_eq!(level(&solo(&["track:b"])), 1.0, "solo silences the other tracks");
    assert_eq!(level(&solo(&["track:c"])), 1.25, "a soloed track keeps its send's return (solo-safe)");
    assert_eq!(level(&solo(&["bus:g"])), 0.5, "a soloed bus plays what feeds it");
    assert_eq!(level(&solo(&["bus:r"])), 0.25, "a soloed return plays only what is sent to it");
    assert_eq!(level(&solo(&["track:a", "track:b"])), 1.5, "solos add up");
    let mut muted = solo(&["track:b"]);
    muted["tracks"][1]["mute"] = json!(true);
    assert_eq!(level(&muted), 0.0, "mute wins over solo on the same strip");
}

#[test]
fn a_latent_insert_is_compensated_so_parallel_paths_stay_aligned() {
    let (reference, _) = reference();
    // Track a runs through 257 frames of latency, track b does not: the mixer delays b.
    let mut latent = project(2);
    latent["audioPlugins"] = json!([effect("track:a", 0, "lat", "test:latency:257")]);
    let engine = engine();
    engine.publish(&parse(&latent)).unwrap();
    engine.play(&st_audio::PlayOptions { from_start: true, ..Default::default() });
    let (left, _) = engine.render_offline(FRAMES, 512);
    assert_signal(&left, |i| if i < 257 { 0.0 } else { reference[i - 257] * 2.0 }, 1e-6, "two aligned tracks, 257 frames late");
    engine.tick();
    assert_eq!(engine.status()["latencyCompensation"]["frames"], 257);
    assert_eq!(engine.inserts()[0].latency_frames, 257);

    // A send into a latent return: the dry path waits for the wet one.
    let mut wet = project(1);
    wet["routing"]["buses"] = json!([bus("r", "return", 1.0)]);
    wet["routing"]["sends"] = json!([{ "id": "s", "source": "track:a", "tap": "postFader", "destination": "bus:r", "gain": 0.5 }]);
    wet["audioPlugins"] = json!([effect("bus:r", 0, "lat", "test:latency:100")]);
    assert_signal(&render(&wet, FRAMES).0, |i| if i < 100 { 0.0 } else { reference[i - 100] * 1.5 }, 1e-6, "dry and wet aligned");

    // Latency inside a group: the group's other track is delayed at the group's input, and a
    // track outside the group at the master's.
    let mut grouped = project(3);
    grouped["routing"]["buses"] = json!([bus("g", "group", 1.0)]);
    grouped["routing"]["outputs"] = json!({ "track:a": "bus:g", "track:b": "bus:g" });
    grouped["audioPlugins"] = json!([effect("track:a", 0, "lat", "test:latency:64"), effect("bus:g", 0, "lat2", "test:latency:36")]);
    assert_signal(&render(&grouped, FRAMES).0, |i| if i < 100 { 0.0 } else { reference[i - 100] * 3.0 }, 2e-6, "three paths aligned at 100 frames");
}

#[test]
fn a_sidechain_reaches_its_insert_and_is_not_heard_by_itself() {
    let (reference, _) = reference();
    // Track a is multiplied by |key| of track b (the test "key" insert); b itself is disconnected.
    let mut keyed = project(2);
    keyed["routing"]["outputs"] = json!({ "track:b": null });
    keyed["routing"]["sidechains"] = json!([{
        "id": "k", "source": "track:b", "tap": "postFader", "gain": 1, "enabled": true,
        "destination": "key",
    }]);
    keyed["audioPlugins"] = json!([effect("track:a", 0, "key", "test:key")]);
    assert_signal(&render(&keyed, FRAMES).0, |i| reference[i] * reference[i].abs(), 1e-6, "keyed by track b");
    // Solo on a: the detector still hears b, although b's audible paths are closed.
    keyed["tracks"][0]["solo"] = json!(true);
    assert_signal(&render(&keyed, FRAMES).0, |i| reference[i] * reference[i].abs(), 1e-6, "solo keeps the key");
    // Without the sidechain the key insert hears silence.
    keyed["routing"]["sidechains"] = json!([]);
    assert_eq!(peak(&render(&keyed, FRAMES).0), 0.0);
}

#[test]
fn the_built_in_sends_feed_the_delay_and_reverb_returns() {
    let (reference, _) = reference();
    let mut delayed = project(1);
    delayed["delay"] = json!({ "time": 0.05, "feedback": 0, "mix": 0.5 });
    delayed["tracks"][0]["delaySend"] = json!(0.5);
    // Dry plus the tone 2400 frames later at send 0.5 x return 0.5.
    assert_signal(&render(&delayed, FRAMES).0, |i| reference[i] + if i < 2400 { 0.0 } else { reference[i - 2400] * 0.25 }, 1e-4, "delay send");
    let mut reverbed = project(1);
    reverbed["tracks"][0]["reverbSend"] = json!(1.0);
    let (left, _) = render(&reverbed, FRAMES);
    let difference: Vec<f32> = left.iter().zip(&reference).map(|(wet, dry)| wet - dry).collect();
    assert!(peak(&difference) > 0.01, "the reverb return is audible");
    reverbed["reverb"] = json!({ "mix": 0 });
    assert_signal(&render(&reverbed, FRAMES).0, |i| reference[i], 0.0, "a return at 0 adds nothing");
}

#[test]
fn automation_lands_on_its_frame_and_ramps_in_control_steps() {
    let (reference, _) = reference();
    let gain_at = |left: &[f32], frame: usize| -> f32 {
        // Read the gain where the reference is loud enough to divide by.
        let at = (frame..frame + 100).find(|index| reference[*index].abs() > 0.1).expect("a loud frame");
        left[at] / reference[at]
    };
    // Master volume, stepped: 1 from row 0, 0.25 from row 2.
    let mut stepped = project(1);
    stepped["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "master", "target": "volume", "interpolation": "step", "points": [{ "row": 0, "value": 1.0 }, { "row": 2, "value": 0.25 }] }]);
    let (left, _) = render(&stepped, FRAMES);
    assert_signal(&left, |i| reference[i] * if i < 2 * ROW { 1.0 } else { 0.25 }, 1e-6, "a stepped lane changes at its exact frame");

    // A bus fader, linear from 0 at row 0 to 1 at row 4: a staircase of 32-frame steps on the line.
    let mut ramp = project(1);
    ramp["routing"]["buses"] = json!([bus("g", "group", 1.0)]);
    ramp["routing"]["outputs"] = json!({ "track:a": "bus:g" });
    ramp["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "bus:g", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.0 }, { "row": 4, "value": 1.0 }] }]);
    let (left, _) = render(&ramp, FRAMES);
    for frame in [0usize, 3_200, 9_600, 16_000, 20_800, 23_968] {
        let line = frame as f32 / (4 * ROW) as f32;
        for inside in [frame, frame + 13, frame + 31] {
            if reference[inside].abs() > 0.05 {
                assert!((left[inside] / reference[inside] - line).abs() < 1e-4, "frame {inside}: gain {} for a line value of {line}", left[inside] / reference[inside]);
            }
        }
    }
    assert_eq!(left[0], 0.0, "the ramp starts at zero");

    // Track volume belongs to the engine's sequencer: its lane arrives as timed events.
    let mut track = project(1);
    track["patterns"][0]["automation"] = json!([{ "id": "l", "track": 0, "target": "volume", "interpolation": "step", "points": [{ "row": 0, "value": 1.0 }, { "row": 1, "value": 0.5 }, { "row": 3, "value": 2.0 }] }]);
    let (left, _) = render(&track, FRAMES);
    assert_signal(&left, |i| reference[i] * if i < ROW { 1.0 } else if i < 3 * ROW { 0.5 } else { 2.0 }, 1e-6, "track volume lane");
    // The same for a step effect: volume 0.25 on row 2 holds until something changes it.
    let mut locked = project(1);
    locked["patterns"][0]["steps"][0][2] = json!({ "fx1": { "type": "volume", "value": 0.25 } });
    let (left, _) = render(&locked, FRAMES);
    assert!((gain_at(&left, ROW) - 1.0).abs() < 1e-6 && (gain_at(&left, 2 * ROW) - 0.25).abs() < 1e-6);

    // A send's gain, by route id.
    let mut send = project(1);
    send["routing"]["buses"] = json!([bus("r", "return", 1.0)]);
    send["routing"]["outputs"] = json!({ "track:a": null });
    send["routing"]["sends"] = json!([{ "id": "s", "source": "track:a", "tap": "postFader", "destination": "bus:r", "gain": 1.0 }]);
    send["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "track:a", "target": "send:s", "interpolation": "step", "points": [{ "row": 0, "value": 0.5 }, { "row": 1, "value": 1.5 }] }]);
    let (left, _) = render(&send, FRAMES);
    assert_signal(&left, |i| reference[i] * if i < ROW { 0.5 } else { 1.5 }, 1e-6, "send gain lane");
}

#[test]
fn insert_parameters_follow_lanes_and_step_locks_and_return_to_their_base() {
    let (reference, _) = reference();
    // The test gain insert multiplies by twice parameter 0; its base value 0.5 is unity.
    let mut lane = project(1);
    lane["audioPlugins"] = json!([effect("track:a", 0, "gain", "test:gain")]);
    lane["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "track:a", "target": "plugin:effect:0:0", "interpolation": "step", "points": [{ "row": 0, "value": 0.5 }, { "row": 1, "value": 0.25 }, { "row": 2, "value": 1.0 }] }]);
    let (left, _) = render(&lane, FRAMES);
    assert_signal(&left, |i| reference[i] * if i < ROW { 1.0 } else if i < 2 * ROW { 0.5 } else { 2.0 }, 1e-6, "insert parameter lane");

    // A step lock holds for its row; the next row returns to the base value.
    let mut lock = project(1);
    lock["audioPlugins"] = json!([effect("track:a", 0, "gain", "test:gain")]);
    lock["patterns"][0]["steps"][0][1] = json!({ "fx1": { "type": "plugin", "instanceID": "gain", "address": "0", "value": 0.125 } });
    let (left, _) = render(&lock, FRAMES);
    assert_signal(&left, |i| reference[i] * if (ROW..2 * ROW).contains(&i) { 0.25 } else { 1.0 }, 1e-6, "plug-in step lock");

    // An instrument's insert runs on every track that plays the instrument, and its lane reaches each.
    let mut chain = project(2);
    chain["audioPlugins"] = json!([effect("instrument:0", 0, "igain", "test:gain")]);
    chain["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "instrument:0", "target": "plugin:effect:0:0", "interpolation": "step", "points": [{ "row": 0, "value": 0.25 }] }]);
    let (left, _) = render(&chain, FRAMES);
    assert_signal(&left, |i| reference[i] * 1.0, 2e-6, "two tracks at half gain each");

    // Bypass, and a saved state as the base value.
    let mut bypassed = project(1);
    bypassed["audioPlugins"] = json!([effect("track:a", 0, "gain", "test:gain")]);
    bypassed["audioPlugins"][0]["state"] = json!("0.25");
    assert_signal(&render(&bypassed, FRAMES).0, |i| reference[i] * 0.5, 1e-6, "saved state");
    bypassed["audioPlugins"][0]["bypass"] = json!(true);
    assert_signal(&render(&bypassed, FRAMES).0, |i| reference[i], 0.0, "bypassed");
}

#[test]
fn built_in_effects_are_heard_and_plug_ins_pass_through_as_unavailable() {
    let (reference, _) = reference();
    let mut changed = 0;
    let mut total = 0;
    for descriptor in tracker_engine::effects::catalogue() {
        let id = descriptor.info.id().to_string_lossy().into_owned();
        let mut with = project(1);
        with["audioPlugins"] = json!([effect("track:a", 0, "fx", &format!("stfx:{id}"))]);
        let engine = engine();
        engine.publish(&parse(&with)).unwrap();
        engine.play(&st_audio::PlayOptions { from_start: true, ..Default::default() });
        let (left, right) = engine.render_offline(FRAMES, 512);
        assert!(left.iter().chain(&right).all(|sample| sample.is_finite()), "{id} renders finite audio");
        let status = &engine.inserts()[0];
        assert!(status.available, "{id} is available");
        let latency = status.latency_frames as usize;
        let differs = left.iter().enumerate().skip(latency).any(|(index, sample)| (sample - reference[index - latency]).abs() > 1e-3);
        changed += differs as usize;
        total += 1;
        // Bypassed, the effect is out of the path (its latency stays compensated).
        with["audioPlugins"][0]["bypass"] = json!(true);
        let (bypassed, _) = render(&with, FRAMES);
        assert_signal(&bypassed, |i| if i < latency { 0.0 } else { reference[i - latency] }, 1e-6, &format!("{id} bypassed"));
    }
    assert!(total > 10 && changed * 2 > total, "{changed} of {total} built-in effects change the tone at their defaults");

    let mut plugin = project(1);
    plugin["audioPlugins"] = json!([effect("track:a", 0, "p", "com.vendor.reverb")]);
    let engine = engine();
    engine.publish(&parse(&plugin)).unwrap();
    engine.play(&st_audio::PlayOptions { from_start: true, ..Default::default() });
    assert_signal(&engine.render_offline(FRAMES, 512).0, |i| reference[i], 0.0, "a plug-in insert passes through");
    let status = &engine.inserts()[0];
    assert!(!status.available && status.reason.as_deref().unwrap().contains("not hosted yet"));
    assert_eq!(engine.routing_status()["inserts"][0]["available"], false);
}

#[test]
fn the_metronome_goes_through_the_master() {
    let mut click = project(1);
    click["tracks"][0]["mute"] = json!(true);
    click["metronome"] = json!(true);
    click["metronomeVolume"] = json!(0.5);
    let (left, _) = render(&click, FRAMES);
    assert!(peak(&left[..1440]) > 0.1, "a click on the first beat");
    click["masterVolume"] = json!(0.5);
    let (half, _) = render(&click, FRAMES);
    assert_signal(&half, |i| left[i] * 0.5, 1e-7, "the master fader scales the click");
}

#[test]
fn a_new_plan_while_playing_keeps_tails_delays_and_insert_state() {
    // Changing the mixer under a playing song must not disturb what is already sounding. Here
    // the change is one that cannot be heard (a new, unconnected return bus), so the render must
    // be bit-identical to an undisturbed one although the plan was replaced in the middle.
    let mut playing = project(3);
    playing["routing"]["buses"] = json!([bus("g", "group", 0.7)]);
    playing["routing"]["outputs"] = json!({ "track:a": "bus:g" });
    playing["tracks"][0]["delaySend"] = json!(0.5);
    playing["tracks"][1]["reverbSend"] = json!(0.5);
    playing["delay"] = json!({ "time": 0.11, "feedback": 0.6, "mix": 0.5 });
    playing["audioPlugins"] = json!([effect("track:b", 0, "lat", "test:latency:333"), effect("bus:g", 0, "comp", "stfx:compressor"), effect("master", 0, "phase", "stfx:phaser")]);
    playing["patterns"][0]["automation"] = json!([{ "id": "l", "owner": "bus:g", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.2 }, { "row": 8, "value": 0.9 }] }]);
    let undisturbed = render(&playing, 2 * FRAMES);

    let engine = engine();
    engine.publish(&parse(&playing)).unwrap();
    engine.play(&st_audio::PlayOptions { from_start: true, ..Default::default() });
    let (mut left, mut right) = engine.render_offline(FRAMES, 512);
    let mut changed = playing.clone();
    changed["routing"]["buses"].as_array_mut().unwrap().push(bus("spare", "return", 1.0));
    engine.publish(&parse(&changed)).unwrap();
    assert_eq!(engine.status()["routing"]["strips"].as_array().unwrap().len(), 6, "the mixer was recompiled");
    let (more_left, more_right) = engine.render_offline(FRAMES, 512);
    left.extend(more_left);
    right.extend(more_right);
    assert!(peak(&left) > 0.05);
    assert!(left == undisturbed.0 && right == undisturbed.1, "the plan swap changed the audio");
}

/// How many notes of a track sound at once is the track's `voiceMode`, and nothing else's:
/// without one the notes of a track overlap, on a mono track a note cuts the one before it, and
/// a mono track beside another leaves the other as it is.
#[test]
fn a_tracks_voice_mode_decides_whether_its_notes_overlap() {
    // A held note on row 0 and a second one on row 2, while the first still sounds.
    let second_row = 2 * ROW;
    let held = json!({ "note": 57, "instrument": 0, "velocity": 100, "gate": 8 });
    let notes = |tracks: usize, first: bool, second: bool| {
        let mut project = project(tracks);
        for track in 0..tracks {
            project["patterns"][0]["steps"][track][0] = if first { held.clone() } else { json!(null) };
            project["patterns"][0]["steps"][track][2] = if second { held.clone() } else { json!(null) };
        }
        project
    };
    let (first, _) = render(&notes(1, true, false), FRAMES);
    let (second, _) = render(&notes(1, false, true), FRAMES);
    assert!(peak(&first[second_row..]) > 0.2 && peak(&second[second_row..]) > 0.2 && peak(&second[..second_row]) == 0.0);

    let (overlapping, _) = render(&notes(1, true, true), FRAMES);
    assert_signal(&overlapping, |i| first[i] + second[i], 1e-5, "without a voice mode both notes sound");
    let mut poly = notes(1, true, true);
    poly["tracks"][0]["voiceMode"] = json!("poly");
    assert_eq!(render(&poly, FRAMES).0, overlapping, "poly is what a track without a voice mode is");

    // The cut voice is gone after a short fade; from there the track is its second note alone.
    let settled = second_row + 2_400;
    let mut mono = notes(1, true, true);
    mono["tracks"][0]["voiceMode"] = json!("mono");
    let (cut, _) = render(&mono, FRAMES);
    assert_signal(&cut[..second_row], |i| first[i], 1e-5, "the first note until the second");
    assert_signal(&cut[settled..], |i| second[settled + i], 1e-5, "a mono track plays its latest note alone");

    let mut beside = notes(2, true, true);
    beside["tracks"][0]["voiceMode"] = json!("mono");
    let (both, _) = render(&beside, FRAMES);
    assert_signal(&both[settled..], |i| second[settled + i] + overlapping[settled + i], 1e-5, "the track beside a mono track still overlaps its notes");
}
