//! Headless libmpv regression. Run with `cargo test --test player_dimensions -- --nocapture`.
//! Requires FFmpeg and libmpv with software rendering. All media is generated locally.

use nt_core::{Error, PixelFormat, Player, Status};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_DIR: &str = "NT_TEST_PLAYER_DIMENSIONS_DIR";

#[test]
fn player_dimensions() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = std::env::var_os(CHILD_DIR) {
        return exercise_player(Path::new(&dir));
    }
    match Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .status()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Skipping player_dimensions: ffmpeg is unavailable");
            return Ok(());
        }
        status => assert!(status?.success()),
    }

    // Player settings and fixture paths are process-wide.
    let dir = tempfile::tempdir()?;
    let status = Command::new(std::env::current_exe()?)
        .args(["--exact", "player_dimensions", "--nocapture"])
        .env(CHILD_DIR, dir.path())
        .env("NT_FIXTURES", dir.path())
        .env("NT_FIXTURE_MODE", "replay")
        .env("NT_DATA_DIR", dir.path())
        .env("NT_MAX_HEIGHT", "360")
        .env("NT_MUTE", "1")
        .env("NT_NO_MPRIS", "1")
        .env("NT_MPV_OPTS", "ao=null,hwdec=no")
        .env_remove("NT_VIDEO_OVERRIDE")
        .env_remove("NT_LIVE")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .status()?;
    assert!(status.success());
    Ok(())
}

fn ffmpeg(dir: &Path, source: &str, codec: &[&str], output: &str) -> std::io::Result<()> {
    let status = Command::new("ffmpeg")
        .current_dir(dir)
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", source])
        .args(["-t", "5", "-threads", "2"])
        .args(codec)
        .arg(output)
        .status()?;
    assert!(status.success());
    Ok(())
}

fn fixture(dir: &Path, id: &str, path: &str) -> std::io::Result<()> {
    let key = format!("POST player VISIONOS {id}");
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let reply = serde_json::json!({
        "playabilityStatus": {"status": "OK"},
        "videoDetails": {"title": id},
        "streamingData": {"adaptiveFormats": [{
            "url": dir.join(path),
            "height": 360,
            "mimeType": "video/mp4; codecs=\"avc1.64000C\""
        }]}
    });
    std::fs::write(dir.join(format!("{hash:016x}.bin")), reply.to_string())
}

fn exercise_player(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let player = match Player::new(PixelFormat::Rgba, || {}) {
        Ok(player) => player,
        Err(Error::Player(msg)) if msg.starts_with("libmpv is not installed") => {
            eprintln!("Skipping player_dimensions: {msg}");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let clips = [
        ("first", (320, 144), "red", 0),
        ("same-height", (384, 144), "lime", 1),
        ("same-width", (384, 192), "blue", 2),
        ("same-size", (384, 192), "red", 0),
        ("changed-size", (480, 160), "lime", 1),
    ];
    for (id, (w, h), color, _) in clips {
        let file = format!("{id}.mp4");
        ffmpeg(
            dir,
            &format!("color={color}:size={w}x{h}:rate=24"),
            &["-c:v", "mpeg4"],
            &file,
        )?;
        fixture(dir, id, &file)?;
    }
    ffmpeg(dir, "anullsrc=r=8000:cl=mono", &[], "audio.wav")?;
    // Advertise a video input so the normal resolver hands an audio-only file to mpv.
    fixture(dir, "audio-only", "audio.wav")?;

    for (id, size, _, channel) in clips {
        player.load(id)?;
        check_video(&player, id, size, channel)?;
    }

    player.stop();
    assert_eq!(player.state().status, Status::Idle);
    assert_eq!(player.state().video_size, None);
    assert!(player.frame().is_none());
    player.load("changed-size")?;
    check_video(&player, "changed-size", (480, 160), 1)?;

    player.load("audio-only")?;
    wait_for_playback(&player, "audio-only", || {
        assert_eq!(player.state().video_size, None);
        assert!(player.frame().is_none());
        true
    });
    assert_eq!(player.state().aspect(), None);
    println!("audio-only: no dimensions or frames");

    player.load("first")?;
    check_video(&player, "first", (320, 144), 0)?;

    // Supersede a load while its lifecycle events may still be queued.
    player.load("changed-size")?;
    player.load("same-width")?;
    check_video(&player, "same-width", (384, 192), 2)?;
    Ok(())
}

fn wait_for_playback(player: &Player, id: &str, mut check_output: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = player.state();
        if state.status == Status::Ready && state.source.as_deref() == Some(id) {
            let has_output = check_output();
            if has_output && state.position >= 0.25 {
                return;
            }
        }
        assert!(
            Instant::now() < deadline && !matches!(state.status, Status::Failed(_)),
            "{id}: {state:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn check_video(
    player: &Player,
    id: &str,
    size: (u32, u32),
    channel: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    wait_for_playback(player, id, || {
        let Some(frame) = player.frame() else {
            return false;
        };
        assert_eq!(
            (frame.width, frame.height),
            size,
            "{id}: unexpected frame {frame:?}"
        );
        let center = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
        assert!(frame.pixels()[center + channel] > 200, "{id}: wrong video");
        true
    });
    let state = player.state();
    let frame = player.frame().ok_or("no rendered frame")?;
    println!(
        "{id}: {:?}, {frame:?}, {} bytes",
        state.video_size,
        frame.pixels().len()
    );
    assert_eq!(state.video_size, Some(size), "{id}");
    assert_eq!(state.aspect(), Some(size.0 as f32 / size.1 as f32), "{id}");
    assert_eq!((frame.width, frame.height), size, "{id}");
    assert_eq!(frame.pixels().len(), (size.0 * size.1 * 4) as usize);
    Ok(())
}
