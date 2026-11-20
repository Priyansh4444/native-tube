//! Continuation retries (docs/core-api-additions.md, C3), as far as the public API shows them.
//!
//! Replay-only continuation coverage. A fixture miss must return to the caller and
//! leave the same cursor reusable once the response is present. The generated model
//! compares observed results with the set of cursors currently recorded.

use nt_core::{Continuation, Error, client};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const CHILD: &str = "NT_PROPTEST_RETRY_FIXTURES";
fn fixture_path(dir: &Path, key: &str) -> PathBuf {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    dir.join(format!("{hash:016x}.bin"))
}

/// The recorded continuation answer for `token`: one video whose id names the token.
fn answer(token: &str) -> Value {
    let tail: String = token
        .chars()
        .rev()
        .take(11)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let id = format!("{tail:0>11}");
    json!({"onResponseReceivedCommands":[{"appendContinuationItemsAction":{"continuationItems":[
        {"videoRenderer":{"videoId": id, "title":{"simpleText": format!("Video {token}")}}}
    ]}}]})
}

/// A recorded (empty) page of comments.
fn comments_answer() -> Value {
    json!({"onResponseReceivedEndpoints":[{"appendContinuationItemsAction":{"continuationItems":[]}}]})
}

#[derive(Clone, Debug)]
enum Op {
    /// Ask for the page behind cursor n (videos, or comments on the `next` endpoint).
    Ask { cursor: usize, comments: bool },
    /// The answer for cursor n gets recorded (YouTube recovered; the Retry row is pressed later).
    Record(usize),
}

fn ops() -> impl Strategy<Value = (Vec<String>, Vec<Op>)> {
    (
        prop::collection::btree_set("[A-Za-z0-9_-]{11,40}", 1..5),
        prop::collection::vec(
            prop_oneof![
                3 => (0usize..5, any::<bool>()).prop_map(|(cursor, comments)| Op::Ask { cursor, comments }),
                1 => (0usize..5).prop_map(Op::Record),
            ],
            1..16,
        ),
    )
        .prop_map(|(tokens, ops)| (tokens.into_iter().collect(), ops))
}

#[test]
fn unrecorded_continuations_fail_and_stay_reusable() {
    let Ok(dir) = std::env::var(CHILD) else {
        let dir = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unrecorded_continuations_fail_and_stay_reusable",
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
        cases: (ProptestConfig::default().cases / 4).max(1),
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let result = runner.run(&ops(), |(tokens, operations)| {
        let path = |endpoint, token| {
            let key = format!("POST {endpoint} {{\"continuation\":\"{token}\"}}");
            fixture_path(&dir, &key)
        };
        // A shrink candidate often shares tokens with the previous case.
        for token in &tokens {
            for endpoint in ["search", "next"] {
                let path = path(endpoint, token);
                if let Err(error) = fs::remove_file(path) {
                    prop_assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
                }
            }
        }
        let mut recorded = HashSet::new();
        for op in &operations {
            match *op {
                Op::Record(n) => {
                    let token = &tokens[n % tokens.len()];
                    for (endpoint, body) in [("search", answer(token)), ("next", comments_answer())]
                    {
                        fs::write(path(endpoint, token), body.to_string()).unwrap();
                    }
                    recorded.insert(token.clone());
                }
                Op::Ask { cursor, comments } => {
                    let token = &tokens[cursor % tokens.len()];
                    let endpoint = if comments { "next" } else { "search" };
                    let cursor: Continuation =
                        serde_json::from_value(json!({"endpoint": endpoint, "token": token}))
                            .unwrap();
                    let result: Result<Vec<String>, _> = if comments {
                        client().more_comments(&cursor).wait().map(|page| {
                            page.items.into_iter().map(|comment| comment.text).collect()
                        })
                    } else {
                        client()
                            .more_videos(&cursor)
                            .wait()
                            .map(|page| page.items.into_iter().map(|video| video.title).collect())
                    };
                    if recorded.contains(token) {
                        let expected = if comments {
                            Vec::new()
                        } else {
                            vec![format!("Video {token}")]
                        };
                        prop_assert_eq!(result, Ok(expected), "{:?}: the recorded page", op);
                    } else {
                        prop_assert!(
                            matches!(&result, Err(Error::Fixture(_))),
                            "{:?}: {:?}",
                            op,
                            result
                        );
                    }
                }
            }
        }
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }
}
