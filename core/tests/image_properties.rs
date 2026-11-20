//! The image cache against a reference LRU model.
//!
//! Every decoded image here is 32 MiB, so the 96 MiB budget holds exactly three. Keys 0-2 are
//! always recorded; keys 3-6 come and go (a recorded answer appears or is withdrawn), so a load
//! of them can fail and later succeed. After every operation the cache must hold exactly the
//! model's entries, account exactly their bytes, stay within budget, and a failure must leave
//! nothing behind (the next load tries again) and evict nothing else.

use image::{
    ColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use nt_core::images::{cached_bytes, image};
use proptest::prelude::*;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

const BUDGET: usize = 96 * 1024 * 1024;
const SLOT: usize = 32 * 1024 * 1024;
const KEYS: u8 = 7;
const CHILD: &str = "NT_PROPTEST_IMAGE_FIXTURES";

fn url(key: u8) -> String {
    format!("https://fixture.invalid/property/{key}")
}

/// Where `NT_FIXTURES` replay looks for `GET <url>` (the FNV-1a hash of the key).
fn fixture_path(dir: &Path, key: u8) -> PathBuf {
    let hash = format!("GET {}", url(key))
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    dir.join(format!("{hash:016x}.bin"))
}

/// Makes `key` recorded or not by moving its file in and out of place.
#[expect(
    clippy::unwrap_used,
    reason = "test data lives in an exclusively owned fixture directory"
)]
fn set_available(dir: &Path, key: u8, available: bool) {
    let on = fixture_path(dir, key);
    let off = on.with_extension("off");
    if available && off.exists() {
        fs::rename(off, on).unwrap();
    } else if !available && on.exists() {
        fs::rename(on, off).unwrap();
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Ask for the image; with `duplicate`, ask again before the first answer arrives.
    Load {
        key: u8,
        duplicate: bool,
    },
    /// Two cache misses started before either result is consumed.
    Overlap(u8, u8),
    Publish(u8),
    Withdraw(u8),
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        5 => (0..KEYS, any::<bool>()).prop_map(|(key, duplicate)| Op::Load { key, duplicate }),
        2 => (3..KEYS, 1..KEYS - 3).prop_map(|(first, offset)| {
            Op::Overlap(first, 3 + (first - 3 + offset) % (KEYS - 3))
        }),
        1 => (3..KEYS).prop_map(Op::Publish),
        1 => (3..KEYS).prop_map(Op::Withdraw),
    ];
    prop::collection::vec(op, 1..14)
}

#[test]
fn fixture_cache_matches_reference_lru() {
    // The cache and the fixture directory are process-wide, so the cases run in a child
    // process that owns both.
    let Ok(dir) = std::env::var(CHILD) else {
        let dir = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter)
            .write_image(&vec![0; SLOT], 4096, 2048, ColorType::Rgba8.into())
            .unwrap();
        for key in 0..KEYS {
            fs::write(fixture_path(dir.path(), key), &png).unwrap();
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fixture_cache_matches_reference_lru",
                "--nocapture",
            ])
            .env(CHILD, dir.path())
            .env("NT_FIXTURES", dir.path())
            .env("NT_FIXTURE_MODE", "replay")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: (ProptestConfig::default().cases / 20).max(1),
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let result = runner.run(&ops(), |operations| {
        // Every case starts from the same state: 3 and 4 recorded, 5 and 6 not, and the
        // cache holding exactly 0, 1, 2 (three fresh loads push anything else out).
        let mut available = [true, true, true, true, true, false, false];
        for key in 3..KEYS {
            set_available(&dir, key, available[usize::from(key)]);
        }
        for key in 0..3 {
            image(&url(key), 0).wait().unwrap();
        }
        let mut model = VecDeque::from([0, 1, 2]);
        prop_assert_eq!(cached_bytes(), 3 * SLOT);
        for op in &operations {
            match *op {
                Op::Publish(key) | Op::Withdraw(key) => {
                    let on = matches!(op, Op::Publish(_));
                    available[usize::from(key)] = on;
                    set_available(&dir, key, on);
                }
                Op::Overlap(first, second) => {
                    if !model.contains(&first) && !model.contains(&second) {
                        let inflight = [
                            (first, image(&url(first), 0)),
                            (second, image(&url(second), 0)),
                        ];
                        for (key, job) in inflight {
                            let outcome = job.wait();
                            let bytes = outcome.as_ref().ok().map(|image| image.pixels.len());
                            let expected = available[usize::from(key)].then_some(SLOT);
                            prop_assert_eq!(bytes, expected, "concurrent request for {}", key);
                            if expected.is_some() {
                                model.push_back(key);
                                while model.len() * SLOT > BUDGET {
                                    model.pop_front();
                                }
                            }
                        }
                    }
                }
                Op::Load { key, duplicate } => {
                    let first = image(&url(key), 0);
                    let second = duplicate.then(|| image(&url(key), 0));
                    let result = first.wait();
                    let cached = model.iter().position(|&entry| entry == key);
                    if cached.is_some() || available[usize::from(key)] {
                        let image =
                            result.map_err(|e| TestCaseError::fail(format!("{op:?}: {e}")))?;
                        prop_assert_eq!(image.pixels.len(), SLOT);
                        if let Some(second) = second {
                            let again = second.wait().map_err(|e| {
                                TestCaseError::fail(format!("{op:?} duplicate: {e}"))
                            })?;
                            prop_assert!(
                                std::sync::Arc::ptr_eq(&image, &again),
                                "{:?}: the duplicate decoded again",
                                op
                            );
                        }
                        if let Some(index) = cached {
                            model.remove(index);
                        }
                        model.push_back(key);
                        while model.len() * SLOT > BUDGET {
                            model.pop_front();
                        }
                    } else {
                        prop_assert!(result.is_err(), "{:?}: loaded without a recording", op);
                        if let Some(second) = second {
                            prop_assert!(
                                second.wait().is_err(),
                                "{:?}: duplicate loaded without a recording",
                                op
                            );
                        }
                    }
                }
            }
            prop_assert_eq!(
                cached_bytes(),
                model.len() * SLOT,
                "after {:?}, model {:?}",
                op,
                model
            );
            prop_assert!(cached_bytes() <= BUDGET);
            // A finished entry answers at once. Reading in the model's LRU order keeps the
            // real order equal to the model's.
            for &present in &model {
                let ready = image(&url(present), 0).try_take();
                prop_assert!(
                    matches!(ready, Some(Ok(_))),
                    "after {:?}: {} missing, model {:?}",
                    op,
                    present,
                    model
                );
            }
        }
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }
}
