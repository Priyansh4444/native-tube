//! Video poster source selection (docs/core-api-additions.md, "Video posters").
//!
//! For a random set of recorded poster variants of one video (each a solid colour, so the
//! decoded pixels tell which file was used), a random poster URL and a random drawn width,
//! `images::image` must pick the first usable variant in the documented order:
//!
//! - at 320 px and below (and not 0), `mqdefault` only;
//! - 321–480 px: `hqdefault`, `sddefault`, signed and canonical HD, then `mqdefault`;
//! - wider or original size: the renderer's own signed HD URL (when it is not the canonical `hq720`), then the
//!   canonical `hq720`, `sddefault` and `hqdefault`, then `mqdefault`;
//! - a variant narrower than 320 px is YouTube's "no such poster" placeholder and is skipped,
//!   except `mqdefault`, the last resort;
//!
//! and return it centre-cropped to 16:9 and scaled to at most the drawn width. URLs that are
//! not video posters (avatars, animated previews) are fetched as given, uncropped.

use image::{ImageEncoder, codecs::png::PngEncoder};
use nt_core::images::image;
use proptest::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const CHILD: &str = "NT_PROPTEST_THUMBNAIL_FIXTURES";

fn fixture_path(dir: &Path, url: &str) -> PathBuf {
    let hash = format!("GET {url}")
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    dir.join(format!("{hash:016x}.bin"))
}

/// The files a poster can come from, in the order the spec tries them when wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Renderer,
    Hd,
    Sd,
    Hq,
    Medium,
    /// The URL itself, for URLs that are not video posters.
    Direct,
}

const SOURCES: [Source; 6] = [
    Source::Renderer,
    Source::Hd,
    Source::Sd,
    Source::Hq,
    Source::Medium,
    Source::Direct,
];

impl Source {
    fn colour(self) -> [u8; 4] {
        match self {
            Source::Renderer => [255, 0, 0, 255],
            Source::Hd => [0, 255, 0, 255],
            Source::Sd => [0, 0, 255, 255],
            Source::Hq => [255, 255, 0, 255],
            Source::Medium => [255, 0, 255, 255],
            Source::Direct => [0, 255, 255, 255],
        }
    }
}

/// The poster URL a frontend got from a list response.
#[derive(Clone, Copy, Debug)]
enum Request {
    /// `hq720.jpg?sqp=...&rs=...`: the signed HD URL search and home renderers carry.
    SignedHq720,
    /// `maxresdefault.jpg`, which some renderers carry instead.
    Maxres,
    CanonicalHq720,
    /// `Video::thumbnail_url`, used when a renderer has no image list.
    Medium,
    /// `vi_webp/<id>/hqdefault.webp`.
    WebpHq,
    /// An animated preview on the same host: not a poster.
    Animated,
    /// A channel avatar on yt3.ggpht.com.
    Avatar,
}

fn request_url(request: Request, id: &str) -> String {
    match request {
        Request::SignedHq720 => {
            format!("https://i.ytimg.com/vi/{id}/hq720.jpg?sqp=-oaymwEc&rs=AOn4CL")
        }
        Request::Maxres => format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"),
        Request::CanonicalHq720 => canonical(id, "hq720"),
        Request::Medium => canonical(id, "mqdefault"),
        Request::WebpHq => format!("https://i.ytimg.com/vi_webp/{id}/hqdefault.webp"),
        Request::Animated => format!("https://i.ytimg.com/an_webp/{id}/mqdefault_6s.webp?du=3000"),
        Request::Avatar => format!("https://yt3.ggpht.com/ytc/{id}=s88-c-k"),
    }
}

fn canonical(id: &str, name: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/{name}.jpg")
}

/// The URL each source is recorded under, if the request has that source at all.
fn source_url(source: Source, request: Request, id: &str) -> Option<String> {
    let poster = !matches!(request, Request::Animated | Request::Avatar);
    match source {
        Source::Direct => (!poster).then(|| request_url(request, id)),
        _ if !poster => None,
        Source::Renderer => matches!(request, Request::SignedHq720 | Request::Maxres)
            .then(|| request_url(request, id)),
        Source::Hd => Some(canonical(id, "hq720")),
        Source::Sd => Some(canonical(id, "sddefault")),
        Source::Hq => Some(canonical(id, "hqdefault")),
        Source::Medium => Some(canonical(id, "mqdefault")),
    }
}

/// The spec: which source answers, and the size it is returned at.
fn expected(
    request: Request,
    width: u32,
    files: &[(Source, Option<(u32, u32)>)],
) -> Option<(Source, u32, u32)> {
    let size = |source| {
        files
            .iter()
            .find(|(s, _)| *s == source)
            .and_then(|(_, size)| *size)
    };
    let order: &[Source] = match request {
        Request::Animated | Request::Avatar => &[Source::Direct],
        _ if width != 0 && width <= 320 => &[Source::Medium],
        Request::SignedHq720 | Request::Maxres if (321..=480).contains(&width) => &[
            Source::Hq,
            Source::Sd,
            Source::Renderer,
            Source::Hd,
            Source::Medium,
        ],
        _ if (321..=480).contains(&width) => &[Source::Hq, Source::Sd, Source::Hd, Source::Medium],
        Request::SignedHq720 | Request::Maxres => &SOURCES[..5],
        _ => &SOURCES[1..5],
    };
    let (source, (w, h)) = order.iter().find_map(|&source| {
        let (w, h) = size(source)?;
        let usable = matches!(source, Source::Medium | Source::Direct) || w >= 320;
        usable.then_some((source, (w, h)))
    })?;
    let mut h = h;
    if source != Source::Direct {
        h = h.min((u64::from(w) * 9 / 16).max(1) as u32);
    }
    if width > 0 && w > width {
        h = (u64::from(h) * u64::from(width) / u64::from(w)).max(1) as u32;
        return Some((source, width, h));
    }
    Some((source, w, h))
}

fn png(width: u32, height: u32, colour: [u8; 4]) -> Result<Vec<u8>, TestCaseError> {
    let pixels: Vec<u8> = colour
        .iter()
        .copied()
        .cycle()
        .take((width * height * 4) as usize)
        .collect();
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| TestCaseError::fail(format!("encode {width}x{height} PNG: {e}")))?;
    Ok(out)
}

fn sizes() -> impl Strategy<Value = Option<(u32, u32)>> {
    prop::option::weighted(
        0.6,
        prop_oneof![
            // The sizes YouTube serves, its placeholder, and anything else.
            prop::sample::select(vec![
                (1280, 720),
                (640, 480),
                (480, 360),
                (320, 180),
                (120, 90)
            ]),
            (1u32..1400, 1u32..900),
        ],
    )
}

fn requests() -> impl Strategy<Value = Request> {
    prop::sample::select(vec![
        Request::SignedHq720,
        Request::Maxres,
        Request::CanonicalHq720,
        Request::Medium,
        Request::WebpHq,
        Request::Animated,
        Request::Avatar,
    ])
}

fn widths() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0u32),
        1u32..=320,
        321u32..2000,
        prop::sample::select(vec![320u32, 321, 480, 481, 640, 1280])
    ]
}

#[test]
fn posters_come_from_the_first_usable_variant() {
    let Ok(dir) = std::env::var(CHILD) else {
        let dir = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "posters_come_from_the_first_usable_variant",
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
    // Each case gets a video id of its own: the core remembers missing variants per id.
    static CASE: AtomicU32 = AtomicU32::new(0);
    let strategy = (
        requests(),
        prop::collection::vec(widths(), 1..4),
        [sizes(), sizes(), sizes(), sizes(), sizes(), sizes()],
    );
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: (ProptestConfig::default().cases / 4).max(1),
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let result = runner.run(&strategy, |(request, drawn_widths, sizes)| {
        let id = format!("pt{:09}", CASE.fetch_add(1, Ordering::Relaxed));
        let files: Vec<(Source, Option<(u32, u32)>)> = SOURCES
            .iter()
            .zip(sizes)
            .map(|(&source, size)| {
                (
                    source,
                    size.filter(|_| source_url(source, request, &id).is_some()),
                )
            })
            .collect();
        for &(source, size) in &files {
            if let (Some(url), Some((w, h))) = (source_url(source, request, &id), size) {
                fs::write(fixture_path(&dir, &url), png(w, h, source.colour())?).unwrap();
            }
        }
        let url = request_url(request, &id);
        for width in drawn_widths {
            let got = image(&url, width).wait();
            match (expected(request, width, &files), got) {
                (None, Err(_)) => {}
                (None, Ok(img)) => {
                    return Err(TestCaseError::fail(format!(
                        "{url} at {width} px: expected an error, got {}x{} {:?}",
                        img.width,
                        img.height,
                        &img.pixels[..4]
                    )));
                }
                (Some(expected), Err(e)) => {
                    return Err(TestCaseError::fail(format!(
                        "{url} at {width} px: expected {expected:?}, got {e}"
                    )));
                }
                (Some((source, w, h)), Ok(img)) => {
                    let centre = ((img.height / 2 * img.width + img.width / 2) * 4) as usize;
                    let pixel = &img.pixels[centre..centre + 4];
                    let close = pixel
                        .iter()
                        .zip(source.colour())
                        .all(|(a, b)| a.abs_diff(b) <= 2);
                    prop_assert!(
                        close,
                        "{} at {} px: expected {:?}, got pixel {:?}",
                        url,
                        width,
                        source,
                        pixel
                    );
                    prop_assert_eq!(
                        (img.width, img.height),
                        (w, h),
                        "{} at {} px from {:?}",
                        url,
                        width,
                        source
                    );
                }
            }
        }
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }
}
