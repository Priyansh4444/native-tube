use nt_core::{ChannelRef, LibraryData, Video, library};
use proptest::prelude::*;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const CHILD: &str = "NT_PROPTEST_LIBRARY";
const FIXTURE: &str = "library.json";

#[expect(
    clippy::unwrap_used,
    reason = "a missing child snapshot is a failure of this test harness"
)]
fn child(dir: &Path, mode: &str) -> LibraryData {
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "library_child", "--nocapture"])
        .env("NT_DATA_DIR", dir)
        .env(CHILD, mode)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let stdout = String::from_utf8(result.stdout).unwrap();
    let snapshot = stdout
        .lines()
        .find_map(|line| line.strip_prefix("SNAPSHOT "))
        .unwrap();
    serde_json::from_str(snapshot).unwrap()
}

#[test]
fn library_child() {
    let Ok(mode) = std::env::var(CHILD) else {
        return;
    };
    let snapshot = library().snapshot();
    println!("SNAPSHOT {}", serde_json::to_string(&snapshot).unwrap());
    if mode == "save" {
        let path = nt_core::data_dir().join(FIXTURE);
        fs::remove_file(&path).unwrap();
        library().set_volume(snapshot.volume);
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "save did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn videos() -> impl Strategy<Value = Vec<Video>> {
    let video = (
        "[A-Za-z0-9_-]{1,11}",
        any::<String>(),
        any::<String>(),
        prop::option::of(any::<String>()),
        prop::option::of(any::<String>()),
        prop::option::of("[0-9]{1,3}:[0-5][0-9]"),
        prop::option::of(any::<String>()),
        any::<bool>(),
    )
        .prop_map(
            |(id, title, channel, avatar, views, duration, snippet, is_live)| Video {
                thumbnail: Video::thumbnail_url(&id),
                id,
                title,
                channel,
                channel_avatar: avatar,
                views,
                duration,
                snippet,
                is_live,
                ..Video::default()
            },
        );
    prop::collection::vec(video, 0..8)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn arbitrary_library_survives_loading_saving_and_reloading(
        history in videos(),
        watch_later in videos(),
        subscriptions in prop::collection::vec(("[a-z]{1,8}", any::<String>(), prop::option::of(any::<String>())), 0..8),
        quarter_volume in 0u16..=400,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let data = LibraryData {
            history,
            watch_later,
            subscriptions: subscriptions.into_iter().map(|(id, name, avatar)| ChannelRef {
                id, name, avatar, ..ChannelRef::default()
            }).collect(),
            volume: f64::from(quarter_volume) / 4.0,
        };
        let expected = serde_json::to_value(&data).unwrap();
        fs::write(dir.path().join(FIXTURE), serde_json::to_vec(&data).unwrap()).unwrap();
        prop_assert_eq!(serde_json::to_value(child(dir.path(), "save")).unwrap(), expected.clone());
        prop_assert_eq!(serde_json::to_value(child(dir.path(), "load")).unwrap(), expected);
    }

    #[test]
    fn unreadable_library_is_kept_byte_for_byte(tail in prop::collection::vec(any::<u8>(), 0..128)) {
        let dir = tempfile::tempdir().unwrap();
        let mut corrupt = vec![b'!'];
        corrupt.extend(tail);
        let path = dir.path().join(FIXTURE);
        fs::write(&path, &corrupt).unwrap();
        let fresh = child(dir.path(), "load");
        prop_assert!(fresh.history.is_empty() && fresh.watch_later.is_empty() && fresh.subscriptions.is_empty());
        prop_assert_eq!(fresh.volume, 80.0);
        prop_assert!(!path.exists());
        let files: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().path()).collect();
        prop_assert_eq!(files.len(), 1);
        prop_assert!(files[0].file_name().unwrap().to_string_lossy().starts_with("library.json.unreadable-"));
        prop_assert_eq!(fs::read(&files[0]).unwrap(), corrupt);
    }
}
