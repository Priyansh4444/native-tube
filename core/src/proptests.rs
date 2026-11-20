//! Property-based tests for the core domains that have an exact specification.
//!
//! Each property states one rule of the shared core and lets proptest search for a
//! counterexample; a failure prints the shrunk input and is saved to
//! `core/tests/proptest-regressions/proptests.txt` so the next run tries it first.
//! See docs/testing/README.md for the full list and why each property matters.
//! The image cache, thumbnail and library properties live in `core/tests/`, because they
//! need a process of their own (environment-configured singletons).

use super::{client, format, innertube, polite, resolve};
use crate::scroll::ScrollAnimator;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use serde_json::{Value, json};
use std::time::Duration;

/// The default case count (256, or `PROPTEST_CASES`) divided by `divisor`, for slow properties.
fn config(divisor: u32) -> ProptestConfig {
    let default = ProptestConfig::default();
    ProptestConfig {
        cases: (default.cases / divisor).max(1),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/proptests.txt",
        ))),
        ..default
    }
}

// ---------------------------------------------------------------------------
// Scroll physics
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum ScrollOp {
    WheelTicks(f64),
    WheelPixels(f64),
    FlingStart,
    Fling(f64),
    CancelFling,
    ScrollBy(f64),
    ScrollTo(f64),
    JumpTo(f64),
    Tick,
    SetMaxOffset(f64),
    SetViewport(f64),
    SetSmooth(bool),
}

/// Any finite f64, biased toward ordinary pixel values but reaching the extremes.
fn any_finite() -> BoxedStrategy<f64> {
    prop_oneof![
        16 => -20_000.0..20_000.0,
        2 => any::<f64>().prop_filter("finite", |f| f.is_finite()),
        1 => prop::sample::select(vec![0.0, -0.0, f64::MAX, f64::MIN, f64::MIN_POSITIVE, f64::EPSILON]),
    ]
    .boxed()
}

/// What a person can produce: deltas and velocities up to a million px (per second).
fn realistic() -> BoxedStrategy<f64> {
    prop_oneof![8 => -5_000.0..5_000.0, 1 => -1e6..1e6].boxed()
}

fn scroll_ops(value: BoxedStrategy<f64>) -> impl Strategy<Value = Vec<(ScrollOp, u32)>> {
    let op = prop_oneof![
        value.clone().prop_map(ScrollOp::WheelTicks),
        value.clone().prop_map(ScrollOp::WheelPixels),
        Just(ScrollOp::FlingStart),
        value.clone().prop_map(ScrollOp::Fling),
        Just(ScrollOp::CancelFling),
        value.clone().prop_map(ScrollOp::ScrollBy),
        value.clone().prop_map(ScrollOp::ScrollTo),
        value.clone().prop_map(ScrollOp::JumpTo),
        Just(ScrollOp::Tick),
        value.clone().prop_map(ScrollOp::SetMaxOffset),
        value.prop_map(ScrollOp::SetViewport),
        any::<bool>().prop_map(ScrollOp::SetSmooth),
    ];
    // Each op happens 0-99 ms after the previous one.
    prop::collection::vec((op, 0u32..100), 1..50)
}

fn apply(scroll: &mut ScrollAnimator, op: &ScrollOp, now: f64) {
    match *op {
        ScrollOp::WheelTicks(v) => scroll.wheel_ticks(v, now),
        ScrollOp::WheelPixels(v) => scroll.wheel_pixels(v, now),
        ScrollOp::FlingStart => scroll.fling_start(now),
        ScrollOp::Fling(v) => scroll.fling(v, now),
        ScrollOp::CancelFling => scroll.cancel_fling(now),
        ScrollOp::ScrollBy(v) => scroll.scroll_by(v, now),
        ScrollOp::ScrollTo(v) => scroll.scroll_to(v, now),
        ScrollOp::JumpTo(v) => scroll.jump_to(v),
        ScrollOp::Tick => {
            scroll.tick(now);
        }
        ScrollOp::SetMaxOffset(v) => scroll.set_max_offset(v),
        ScrollOp::SetViewport(v) => scroll.set_viewport(v),
        ScrollOp::SetSmooth(v) => scroll.set_smooth_scroll(v),
    }
}

fn check_scroll(scroll: &ScrollAnimator, after: &str) -> Result<(), TestCaseError> {
    let max = scroll.max_offset();
    prop_assert!(
        max.is_finite() && max >= 0.0,
        "max_offset = {max} after {after}"
    );
    for (name, value) in [("offset", scroll.offset()), ("target", scroll.target())] {
        prop_assert!(
            (0.0..=max).contains(&value),
            "{name} = {value} (max {max}) after {after}"
        );
    }
    let velocity = scroll.velocity();
    prop_assert!(velocity.is_finite(), "velocity = {velocity} after {after}");
    Ok(())
}

/// Runs the ops, then 2.5 s of 60 Hz frames: offset, target and velocity stay finite and in
/// range throughout, and the animator stops asking for frames.
fn run_scroll(operations: &[(ScrollOp, u32)]) -> Result<(), TestCaseError> {
    let mut scroll = ScrollAnimator::new();
    scroll.set_max_offset(10_000.0);
    scroll.jump_to(5_000.0);
    let mut ms = 0;
    for (op, dt) in operations {
        ms += dt;
        apply(&mut scroll, op, f64::from(ms) / 1000.0);
        check_scroll(&scroll, &format!("{op:?} at {ms} ms"))?;
    }
    // The longest motion is a 2 s fling (`FLING_MAX_DURATION_S`); 151 frames is 2.5 s.
    for frame in 1..=151 {
        scroll.tick(f64::from(ms) / 1000.0 + f64::from(frame) / 60.0);
        check_scroll(&scroll, &format!("frame {frame} after the ops"))?;
    }
    prop_assert!(
        !scroll.needs_frame(),
        "still animating 2.5 s after the last input"
    );
    Ok(())
}

proptest! {
    #![proptest_config(config(1))]

    /// Any finite input sequence: no NaN or infinity, offset in [0, max], converges.
    #[test]
    fn scroll_any_finite_sequence_stays_finite_and_converges(ops in scroll_ops(any_finite())) {
        run_scroll(&ops)?;
    }

    /// The same for the magnitudes real wheels, touchpads and keyboards produce.
    #[test]
    fn scroll_realistic_sequence_stays_finite_and_converges(ops in scroll_ops(realistic())) {
        run_scroll(&ops)?;
    }

    /// Whatever was happening before, `scroll_to(x)` comes to rest exactly at clamp(x, 0, max)
    /// within its 1.5 s maximum duration.
    #[test]
    fn scroll_to_lands_at_the_clamped_target(
        before in scroll_ops(realistic()),
        max in 0.0..100_000.0,
        destination in any_finite(),
    ) {
        let mut scroll = ScrollAnimator::new();
        let mut ms = 0;
        for (op, dt) in &before {
            ms += dt;
            apply(&mut scroll, op, f64::from(ms) / 1000.0);
        }
        scroll.set_max_offset(max);
        let now = f64::from(ms) / 1000.0;
        scroll.scroll_to(destination, now);
        scroll.tick(now + 1.501);
        prop_assert_eq!(scroll.offset(), destination.clamp(0.0, max));
        prop_assert!(!scroll.needs_frame());
    }

    /// A single wheel, touchpad, fling or keyboard input moves the offset only in the
    /// direction of its sign, frame after frame, and moves it at least somewhere.
    #[test]
    fn scroll_gestures_move_in_the_input_direction(
        kind in 0u8..4,
        magnitude in 100.0..5_000.0,
        positive in any::<bool>(),
        smooth in any::<bool>(),
    ) {
        let sign = if positive { 1.0 } else { -1.0 };
        let delta = sign * magnitude;
        let mut scroll = ScrollAnimator::new();
        scroll.set_max_offset(20_000.0);
        scroll.set_viewport(800.0);
        scroll.set_smooth_scroll(smooth);
        let initial = 10_000.0;
        scroll.jump_to(initial);
        match kind {
            0 => scroll.wheel_ticks(delta / 120.0, 0.0),
            1 => scroll.wheel_pixels(delta, 0.0),
            2 => scroll.fling(delta, 0.0),
            _ => scroll.scroll_by(delta, 0.0),
        }
        let mut previous = initial;
        for frame in 1..=151 {
            let next = scroll.tick(f64::from(frame) / 60.0);
            prop_assert!((next - previous) * sign >= 0.0, "frame {frame}: {previous} -> {next}");
            previous = next;
        }
        prop_assert!((previous - initial) * sign > 0.0, "never moved from {initial}");
        prop_assert!(!scroll.needs_frame());
    }
}

proptest! {
    #![proptest_config(config(1))]

    /// C1 loudness gain: YouTube's target minus the track's loudness (or -loudnessDb), the
    /// selected format's values before the response-wide ones, clamped to [-96, 0] dB, so a
    /// louder track never gets more gain and nothing is ever boosted.
    #[test]
    fn normalization_attenuates_without_boosting_and_is_monotonic(
        target in -60_i16..=0,
        quieter in -150_i16..=100,
        increase in 0_i16..=100,
        use_lkfs in any::<bool>(),
        format_override in any::<bool>(),
    ) {
        let gain_for = |loudness| {
            let values = if use_lkfs {
                json!({"loudnessTargetLkfs":target, "trackAbsoluteLoudnessLkfs":loudness})
            } else {
                json!({"loudnessDb":loudness})
            };
            let mut audio = json!({"url":"audio", "mimeType":"audio/mp4", "bitrate":100});
            let config = if format_override {
                audio.as_object_mut().unwrap().extend(values.as_object().unwrap().clone());
                // Competing response-level values expose a wrong priority choice.
                if use_lkfs {
                    json!({"loudnessTargetLkfs":-5, "trackAbsoluteLoudnessLkfs":50})
                } else {
                    json!({"loudnessDb":55})
                }
            } else {
                values
            };
            let mut response = player_response(vec![
                json!({"url":"video", "mimeType":"video/mp4; codecs=\"avc1\"", "height":360}),
                audio,
            ]);
            response["playerConfig"] = json!({"audioConfig": config});
            resolve::pick(&response, 720).unwrap().normalization_gain_db
        };
        let louder = quieter + increase;
        let quiet_gain = gain_for(quieter);
        let loud_gain = gain_for(louder);
        let expected_gain = |loudness: i16| {
            let attenuation = if use_lkfs { target - loudness } else { -loudness };
            f64::from(attenuation.clamp(-96, 0))
        };
        prop_assert!((-96.0..=0.0).contains(&quiet_gain));
        prop_assert!((-96.0..=quiet_gain).contains(&loud_gain));
        prop_assert_eq!(quiet_gain, expected_gain(quieter));
        prop_assert_eq!(loud_gain, expected_gain(louder));
    }
}

// ---------------------------------------------------------------------------
// Fixture keys
// ---------------------------------------------------------------------------

/// JSON as request bodies carry it. No floats: request bodies have none, and serde_json
/// without its `float_roundtrip` feature parses some floats back one ulp off.
fn json_values() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<String>().prop_map(Value::String)
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            // Keys that sort differently from their generation order, including "" and escapes.
            prop::collection::btree_map("[^a-z]?[a-z\"\\\\]{0,4}", inner, 0..6)
                .prop_map(|fields| Value::Object(fields.into_iter().rev().collect()))
        ]
    })
}

fn reverse_objects(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut fields: Vec<_> = map.into_iter().collect();
            fields.reverse();
            Value::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, reverse_objects(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(reverse_objects).collect()),
        leaf => leaf,
    }
}

proptest! {
    #![proptest_config(config(1))]

    /// A recorded answer's key must not depend on object key order (serde_json's
    /// `preserve_order` is on in some app trees), must be valid JSON for the same value, and
    /// canonicalizing it again changes nothing.
    #[test]
    fn canonical_json_is_idempotent_order_free_and_faithful(value in json_values()) {
        let canonical = client::canonical_json(&value);
        let parsed: Value = serde_json::from_str(&canonical).unwrap();
        prop_assert_eq!(&parsed, &value);
        prop_assert_eq!(client::canonical_json(&parsed), canonical.clone());
        prop_assert_eq!(client::canonical_json(&reverse_objects(value)), canonical);
    }
}

// ---------------------------------------------------------------------------
// Stream selection
// ---------------------------------------------------------------------------

fn player_response(formats: Vec<Value>) -> Value {
    json!({"playabilityStatus":{"status":"OK"}, "streamingData":{"adaptiveFormats":formats}})
}

const HEIGHTS: [u64; 7] = [144, 240, 360, 480, 720, 1080, 2160];
const VIDEO_CODECS: [&str; 4] = ["vp9", "vp09.00.40.08", "avc1.640028", "av01.0.08M.08"];

proptest! {
    #![proptest_config(config(1))]

    /// Never AV1; the highest height within the cap, then the higher frame rate, then H.264
    /// over VP9, then bitrate; with nothing under the cap, the smallest height.
    #[test]
    fn video_is_never_av1_and_ranks_resolution_fps_codec(
        cap in 0usize..7,
        choices in prop::collection::vec((0usize..7, any::<bool>(), 0usize..4, 1u32..10), 1..25),
    ) {
        let formats = choices.iter().enumerate().map(|(i, &(height, fast, codec, bitrate))| json!({
            "url": i.to_string(),
            "mimeType": format!("video/webm; codecs=\"{}\"", VIDEO_CODECS[codec]),
            "height": HEIGHTS[height], "fps": if fast { 60 } else { 30 }, "bitrate": bitrate,
        })).collect();
        // The rank the spec gives each format: (height, fps, is H.264, bitrate).
        let rank = |&(height, fast, codec, bitrate): &(usize, bool, usize, u32)| (height, fast, codec == 2, bitrate);
        let eligible: Vec<_> = choices.iter().filter(|choice| choice.2 != 3).collect();
        let result = resolve::pick(&player_response(formats), HEIGHTS[cap] as u32);
        if eligible.is_empty() {
            prop_assert!(result.is_err(), "picked a stream from AV1 only");
            return Ok(());
        }
        let picked = result.unwrap();
        let chosen = choices[picked.video.parse::<usize>().unwrap()];
        prop_assert_ne!(chosen.2, 3, "picked AV1");
        match eligible.iter().filter(|choice| choice.0 <= cap).map(|choice| rank(choice)).max() {
            Some(best) => prop_assert_eq!(rank(&chosen), best),
            None => prop_assert_eq!(chosen.0, eligible.iter().map(|choice| choice.0).min().unwrap()),
        }
        prop_assert_eq!(u64::from(picked.height), HEIGHTS[chosen.0]);
    }

    /// Audio: the original track over a dub (even a better-sounding one), then not the
    /// loudness-compressed (DRC) variant, then AAC over Opus, then bitrate.
    #[test]
    fn audio_prefers_original_then_plain_then_aac_then_bitrate(
        tracks in prop::collection::vec(
            (prop::option::of(any::<bool>()), prop::option::of(any::<bool>()), any::<bool>(), 1u32..500),
            1..10,
        ),
    ) {
        let mut formats: Vec<Value> = tracks.iter().enumerate().map(|(i, &(original, drc, aac, bitrate))| {
            let mut format = json!({
                "url": format!("audio{i}"),
                "mimeType": if aac { "audio/mp4; codecs=\"mp4a.40.2\"" } else { "audio/webm; codecs=\"opus\"" },
                "bitrate": bitrate,
            });
            if let Some(original) = original {
                format["audioTrack"] = json!({"audioIsDefault": original});
            }
            if let Some(drc) = drc {
                format["isDrc"] = json!(drc);
            }
            format
        }).collect();
        formats.push(json!({"url":"video", "mimeType":"video/mp4; codecs=\"avc1\"", "height":360}));
        // A track without audioTrack is the only language there is, so it counts as original.
        let rank = |&(original, drc, aac, bitrate): &(Option<bool>, Option<bool>, bool, u32)|
            (original.unwrap_or(true), !drc.unwrap_or(false), aac, bitrate);
        let picked = resolve::pick(&player_response(formats), 720).unwrap();
        let audio = picked.audio.expect("an audio track was offered");
        let index: usize = audio.strip_prefix("audio").unwrap().parse().unwrap();
        prop_assert_eq!(rank(&tracks[index]), tracks.iter().map(rank).max().unwrap());
    }
}

// ---------------------------------------------------------------------------
// Text formatters
// ---------------------------------------------------------------------------

fn counts() -> impl Strategy<Value = u64> {
    prop_oneof![
        0u64..2_000,
        any::<u64>(),
        (0u32..19).prop_flat_map(|exp| {
            let base = 10u64.pow(exp);
            (base.saturating_sub(1_000)..=base.saturating_add(1_000)).boxed()
        }),
    ]
}

proptest! {
    #![proptest_config(config(1))]

    /// `clock` shows m:ss or h:mm:ss whose fields add back up to the whole seconds.
    #[test]
    fn clock_round_trips_whole_seconds(seconds in 0u64..1_000_000_000, quarter in 0u8..4) {
        let shown = format::clock(seconds as f64 + f64::from(quarter) / 4.0);
        let expected_parts = if seconds < 3_600 { 2 } else { 3 };
        prop_assert_eq!(shown.split(':').count(), expected_parts, "{}", shown);
        let mut total = 0u64;
        for (index, part) in shown.split(':').enumerate() {
            let number = part.parse::<u64>().unwrap();
            if index != 0 {
                prop_assert_eq!(part.len(), 2, "{}", shown);
                prop_assert!(number < 60, "{}", shown);
            }
            total = total * 60 + number;
        }
        prop_assert_eq!(total, seconds);
    }

    /// `short_count` is exact below 1000, otherwise within 5% of the count, with the largest
    /// unit that keeps the number below 1000 (no "1000K").
    #[test]
    fn short_count_is_close_and_uses_the_right_unit(n in counts()) {
        let shown = format::short_count(n);
        if n < 1_000 {
            prop_assert_eq!(shown, n.to_string());
            return Ok(());
        }
        let (number, unit) = shown.split_at(shown.len() - 1);
        let scale = match unit {
            "K" => 1e3,
            "M" => 1e6,
            "B" => 1e9,
            _ => return Err(TestCaseError::fail(format!("{n} shown as {shown:?}"))),
        };
        let value: f64 = number.parse().map_err(|_| TestCaseError::fail(format!("{n} shown as {shown:?}")))?;
        prop_assert!(!number.ends_with(".0"), "{} shown as {:?}", n, shown);
        let error = (value * scale - n as f64).abs() / n as f64;
        prop_assert!(error <= 0.05, "{} shown as {:?} ({:.1}% off)", n, shown, error * 100.0);
        let expected_unit = match n {
            0..=999_499 => "K",
            999_500..=999_499_999 => "M",
            _ => "B",
        };
        prop_assert_eq!(unit, expected_unit, "{} shown as {:?}", n, shown);
        prop_assert!(value >= 1.0 && (unit == "B" || value < 1_000.0), "{} shown as {:?}", n, shown);
    }

    /// `relative_time` says "N units ago" with N >= 1, the right plural, and
    /// N whole units <= age < N + 1 units; under a minute (or negative) is "just now".
    #[test]
    fn relative_time_floors_to_the_largest_unit(age in prop_oneof![
        -100i64..100_000,
        any::<i64>(),
        prop::sample::select(vec![59, 60, 3_599, 3_600, 86_399, 86_400, 604_799, 604_800,
            2_629_799, 2_629_800, 31_557_599, 31_557_600]),
    ]) {
        let shown = format::relative_time(age);
        if age < 60 {
            prop_assert_eq!(shown, "just now");
            return Ok(());
        }
        let words: Vec<&str> = shown.split(' ').collect();
        prop_assert_eq!(words.len(), 3, "{}", shown);
        prop_assert_eq!(words[2], "ago");
        let n: i64 = words[0].parse().unwrap();
        let singular = words[1].strip_suffix('s').unwrap_or(words[1]);
        prop_assert_eq!(words[1].ends_with('s'), n != 1, "{}", shown);
        let units = [("minute", 60), ("hour", 3_600), ("day", 86_400), ("week", 604_800),
            ("month", 2_629_800), ("year", 31_557_600)];
        let position = units.iter().position(|(name, _)| *name == singular);
        prop_assert!(position.is_some(), "{}", shown);
        let position = position.unwrap();
        let unit = units[position].1;
        prop_assert!(n >= 1 && n * unit <= age && age - n * unit < unit, "{} for {} s", shown, age);
        // And it is the largest unit that fits.
        if let Some((_, bigger)) = units.get(position + 1) {
            prop_assert!(age < *bigger, "{} for {} s", shown, age);
        }
    }
}

// ---------------------------------------------------------------------------
// Response parsers on recorded fixtures
// ---------------------------------------------------------------------------

/// Two recorded answers of each parsed kind from `.cache/fixtures` (or `NT_FIXTURES`), plus a
/// minimal search answer so a clean checkout still has something to mutate.
fn fixtures() -> &'static [(String, Value)] {
    use std::{collections::BTreeMap, fs, path::PathBuf, sync::OnceLock};
    static FIXTURES: OnceLock<Vec<(String, Value)>> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let core = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::var_os("NT_FIXTURES")
            .map_or_else(|| core.join("../.cache/fixtures"), PathBuf::from);
        let mut chosen: BTreeMap<(&str, String), Value> = BTreeMap::new();
        for line in fs::read_to_string(dir.join("index.txt"))
            .unwrap_or_default()
            .lines()
        {
            let Some((hash, key)) = line.split_once(' ') else {
                continue;
            };
            let kind = if key.starts_with("POST search {\"params\"") {
                "search"
            } else if key.starts_with("POST next {\"videoId\"") {
                "watch"
            } else if key.starts_with("POST next {\"continuation\"") {
                "comments"
            } else if key.starts_with("POST browse {\"browseId\":\"UC") {
                "channel"
            } else if key.starts_with("POST search {\"continuation\"")
                || key.starts_with("POST browse {\"continuation\"")
            {
                "continuation"
            } else {
                continue;
            };
            if chosen.keys().filter(|(label, _)| *label == kind).count() >= 2 {
                continue;
            }
            let Ok(bytes) = fs::read(dir.join(format!("{hash}.bin"))) else {
                continue;
            };
            if let Ok(body) = serde_json::from_slice(&bytes) {
                chosen.insert((kind, hash.to_string()), body);
            }
        }
        // Checked-in answers, so a clean checkout exercises every parser; local recordings add more.
        let seeds: [(&str, &str, &str); 15] = [
            (
                "search",
                "seed-age-gate",
                include_str!("testdata/web-age-gate.json"),
            ),
            (
                "search",
                "seed-mixed",
                include_str!("testdata/web-search-mixed.json"),
            ),
            (
                "watch",
                "seed-heatmap",
                include_str!("testdata/web-heatmap-entity.json"),
            ),
            (
                "watch",
                "seed-chapters",
                include_str!("testdata/web-chapters.json"),
            ),
            (
                "comments",
                "seed-comments",
                include_str!("testdata/web-comments.json"),
            ),
            (
                "comments",
                "seed-replies",
                include_str!("testdata/web-replies.json"),
            ),
            (
                "channel",
                "seed-tabs",
                include_str!("testdata/web-channel-tabs.json"),
            ),
            (
                "about",
                "seed-about",
                include_str!("testdata/web-channel-about.json"),
            ),
            (
                "playlist",
                "seed-playlist",
                include_str!("testdata/web-playlist.json"),
            ),
            (
                "continuation",
                "seed-search",
                include_str!("testdata/web-search-continuation.json"),
            ),
            (
                "transcripts",
                "seed-transcripts",
                include_str!("testdata/transcript-search.json"),
            ),
            (
                "player",
                "seed-audio",
                include_str!("testdata/visionos-audio-tracks.json"),
            ),
            (
                "player",
                "seed-captions",
                include_str!("testdata/visionos-captions.json"),
            ),
            (
                "player",
                "seed-storyboard",
                include_str!("testdata/visionos-storyboard.json"),
            ),
            (
                "player",
                "seed-loudness",
                include_str!("testdata/visionos-loudness.json"),
            ),
        ];
        for (kind, name, text) in seeds {
            if let Ok(body) = serde_json::from_str(text) {
                chosen.insert((kind, name.into()), body);
            }
        }
        chosen
            .into_iter()
            .map(|((kind, _), body)| (kind.to_string(), body))
            .collect()
    })
}

fn json_paths(value: &Value, prefix: String, paths: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let key = key.replace('~', "~0").replace('/', "~1");
                json_paths(child, format!("{prefix}/{key}"), paths);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                json_paths(child, format!("{prefix}/{index}"), paths);
            }
        }
        _ => {}
    }
    paths.push(prefix);
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Remove,
    Retype,
    Truncate,
    Text,
    Null,
}

fn mutate(value: &mut Value, path: &str, mutation: Mutation, text: String) {
    if let Mutation::Remove = mutation
        && let Some((parent, key)) = path.rsplit_once('/')
        && let Some(fields) = value.pointer_mut(parent).and_then(Value::as_object_mut)
    {
        fields.shift_remove(&key.replace("~1", "/").replace("~0", "~"));
        return;
    }
    let node = value.pointer_mut(path).unwrap();
    match mutation {
        Mutation::Retype => {
            *node = if node.is_boolean() {
                json!({})
            } else {
                json!(true)
            }
        }
        Mutation::Truncate => {
            if let Some(items) = node.as_array_mut() {
                items.truncate(items.len() / 2);
            } else {
                *node = json!([]);
            }
        }
        Mutation::Text => *node = json!(text),
        Mutation::Remove | Mutation::Null => *node = Value::Null,
    }
}

fn check_parse(kind: &str, value: &Value) -> Result<(), TestCaseError> {
    let videos = match kind {
        "search" => innertube::parse_search(value)
            .map(|page| page.items)
            .and_then(|mut items| {
                let page = innertube::search_page(value, "fixture", false)?;
                items.extend(page.results.items);
                items.extend(page.entries.into_iter().filter_map(|entry| match entry {
                    crate::SearchResult::Video(video) => Some(*video),
                    _ => None,
                }));
                Ok(items)
            }),
        "watch" => innertube::parse_watch(value, "fixture").map(|watch| {
            for point in &watch.heatmap {
                assert!(point.start.is_finite() && (0.0..=1.0).contains(&point.intensity));
            }
            watch.related.items
        }),
        "comments" => innertube::parse_comments(value).map(|_| Vec::new()),
        "channel" => innertube::parse_channel(value, "fixture").map(|channel| channel.videos.items),
        "about" => innertube::channel_about(value).map(|_| Vec::new()),
        "playlist" => Ok(innertube::playlist_panel(&value["watch"], None, None)
            .map(|panel| panel.items)
            .unwrap_or_default()),
        "transcripts" => serde_json::to_vec(value)
            .map_err(crate::Error::from)
            .and_then(|bytes| crate::transcripts::parse(&bytes, "fixture", 1))
            .map(|(page, _)| page.items),
        "player" => resolve::pick_with_audio(value, 720, None).map(|streams| {
            if let Some(board) = &streams.storyboard {
                for seconds in [0.0, 1.0, 1e9, -1.0, f64::NAN] {
                    let _ = board.level_for_width(160).frame_at(seconds);
                }
            }
            Vec::new()
        }),
        _ => innertube::parse_video_continuation(value, "browse").map(|page| page.items),
    };
    if let Ok(videos) = videos {
        prop_assert!(
            videos.iter().all(|video| !video.id.is_empty()),
            "a {} parse produced a video with an empty id",
            kind
        );
    }
    Ok(())
}

#[test]
fn every_parser_has_a_checked_in_seed() {
    let kinds: std::collections::BTreeSet<&str> =
        fixtures().iter().map(|(kind, _)| kind.as_str()).collect();
    for kind in [
        "search",
        "watch",
        "comments",
        "channel",
        "about",
        "playlist",
        "continuation",
        "transcripts",
        "player",
    ] {
        assert!(kinds.contains(kind), "no fixture for {kind}");
    }
}

proptest! {
    #![proptest_config(config(2))]

    /// Parsers never panic on a recorded answer with fields removed, retyped, truncated,
    /// replaced by text or nulled, and never return a video without an id.
    #[test]
    fn parsers_survive_mutated_fixtures(
        fixture in any::<prop::sample::Index>(),
        edits in prop::collection::vec((
            any::<prop::sample::Index>(),
            prop::sample::select(vec![Mutation::Remove, Mutation::Retype, Mutation::Truncate, Mutation::Text, Mutation::Null]),
            "[ -~]{0,16}",
        ), 1..8),
    ) {
        let (kind, source) = fixture.get(fixtures());
        let mut value = source.clone();
        for (index, mutation, text) in edits {
            let mut paths = Vec::new();
            json_paths(&value, String::new(), &mut paths);
            mutate(&mut value, index.get(&paths), mutation, text);
        }
        check_parse(kind, &value)?;
    }

    /// Emptying any video id in any recorded answer drops that video instead of returning it.
    #[test]
    fn parsers_never_emit_an_empty_video_id(
        fixture in any::<prop::sample::Index>(),
        index in any::<prop::sample::Index>(),
    ) {
        let (kind, source) = fixture.get(fixtures());
        let mut value = source.clone();
        let mut paths = Vec::new();
        json_paths(&value, String::new(), &mut paths);
        paths.retain(|path| {
            (path.ends_with("/videoId") || path.ends_with("/contentId")) && value.pointer(path).is_some_and(Value::is_string)
        });
        prop_assume!(!paths.is_empty());
        let path = index.get(&paths);
        *value.pointer_mut(path).unwrap() = json!("");
        check_parse(kind, &value).map_err(|error| TestCaseError::fail(format!("{error} (emptied {path})")))?;
    }
}

// ---------------------------------------------------------------------------
// The polite gate
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(4))]

    /// However long requests take, at most 3 are in flight and their starts are at least
    /// 150 ms apart (on Tokio's paused clock, so a case takes no real time).
    #[test]
    fn polite_gate_caps_concurrency_and_spaces_starts(durations in prop::collection::vec(0u64..900, 1..14)) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        let (peak, mut starts) = rt.block_on(async {
            use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
            use std::sync::{Arc, Mutex};
            let active = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let starts = Arc::new(Mutex::new(Vec::new()));
            let mut jobs = tokio::task::JoinSet::new();
            for duration in durations {
                let (active, peak, starts) = (active.clone(), peak.clone(), starts.clone());
                jobs.spawn(async move {
                    let _permit = polite::turn().await.unwrap();
                    starts.lock().unwrap().push(tokio::time::Instant::now());
                    peak.fetch_max(active.fetch_add(1, Relaxed) + 1, Relaxed);
                    tokio::time::sleep(Duration::from_millis(duration)).await;
                    active.fetch_sub(1, Relaxed);
                });
            }
            while let Some(done) = jobs.join_next().await {
                done.unwrap();
            }
            (peak.load(Relaxed), Arc::try_unwrap(starts).unwrap().into_inner().unwrap())
        });
        prop_assert!((1..=3).contains(&peak), "peak in flight was {peak}");
        starts.sort();
        for pair in starts.windows(2) {
            let gap = pair[1] - pair[0];
            prop_assert!(gap >= Duration::from_millis(150), "two starts only {gap:?} apart");
        }
    }
}

/// A signal the gate can receive from a response.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Signal {
    /// An HTTP status, with an optional Retry-After in seconds.
    Status(u16, Option<u64>),
    /// "Sign in to confirm you're not a bot" inside a successful answer.
    BotCheck,
}

impl Signal {
    /// The cooldown the signal must start, if any.
    fn cooldown(&self) -> Option<u64> {
        match *self {
            Signal::Status(429, retry) => Some(retry.unwrap_or(300).max(60)),
            Signal::Status(403, _) => Some(300),
            Signal::BotCheck => Some(1_800),
            Signal::Status(..) => None,
        }
    }
}

fn signals() -> impl Strategy<Value = Vec<(u64, Signal)>> {
    let signal = prop_oneof![
        3 => (Just(429u16), prop::option::of(prop_oneof![0u64..120, 0u64..10_000])).prop_map(|(s, r)| Signal::Status(s, r)),
        2 => Just(Signal::Status(403, None)),
        1 => Just(Signal::BotCheck),
        2 => prop::sample::select(vec![200u16, 204, 404, 500, 503]).prop_map(|s| Signal::Status(s, None)),
    ];
    prop::collection::vec((prop_oneof![Just(0_u64), 1_u64..400], signal), 1..6)
}

const COOLDOWN_CHILD: &str = "NT_PROPTEST_COOLDOWN";

/// Use one paused clock per child process because the real gate is a singleton.
#[test]
fn polite_cooldown_child() {
    let Ok(encoded) = std::env::var(COOLDOWN_CHILD) else {
        return;
    };
    let signals: Vec<(u64, Signal)> = serde_json::from_str(&encoded).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    rt.block_on(async {
        use tokio::time::{Instant, advance, timeout};
        let mut deadline = Instant::now();
        for (gap, signal) in signals {
            advance(Duration::from_secs(gap)).await;
            let now = Instant::now();
            let error = match &signal {
                Signal::Status(code, retry) => {
                    let value = retry
                        .map(|r| reqwest::header::HeaderValue::from_str(&r.to_string()).unwrap());
                    polite::check_status(
                        reqwest::StatusCode::from_u16(*code).unwrap(),
                        value.as_ref(),
                    )
                    .is_err()
                }
                Signal::BotCheck => polite::check_refusal("Sign in to confirm you're not a bot"),
            };
            if let Some(seconds) = signal.cooldown() {
                deadline = deadline.max(now + Duration::from_secs(seconds));
            }
            assert_eq!(
                error,
                signal.cooldown().is_some()
                    || matches!(signal, Signal::Status(code, _) if !(200..300).contains(&code)),
                "{signal:?}"
            );
            let turn = timeout(Duration::from_secs(1), polite::turn())
                .await
                .unwrap();
            assert_eq!(turn.is_err(), now < deadline, "after {signal:?}");
            if now < deadline {
                assert_eq!(Instant::now(), now, "cooldown must reject immediately");
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Some(before) = remaining.checked_sub(Duration::from_millis(1)) {
            advance(before).await;
            assert!(polite::turn().await.is_err(), "ended before deadline");
            advance(Duration::from_millis(1)).await;
            assert!(
                polite::turn().await.is_ok(),
                "gate still blocked at deadline"
            );
        }
    });
}

proptest! {
    #![proptest_config(config(8))]

    /// A 429 (Retry-After, at least 60 s, else 5 min), a 403 (5 min) or a bot check (30 min)
    /// pauses every request at once and until exactly the latest deadline any signal set, on
    /// Tokio's paused clock; other statuses fail only their own request. Each case runs in a
    /// fresh process because the gate is process-wide.
    #[test]
    fn polite_cooldown_takes_the_longest_signal(signals in signals()) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "proptests::polite_cooldown_child", "--nocapture", "--test-threads=1"])
            .env(COOLDOWN_CHILD, serde_json::to_string(&signals).unwrap())
            .output()
            .unwrap();
        prop_assert!(output.status.success(), "child failed: {}{}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}
