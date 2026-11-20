//! Finding a video's streams, in Rust, the way YouTube's own native apps do.
//!
//! YouTube's web player has to run obfuscated JavaScript to unlock its stream
//! URLs, and most clients now need a proof-of-origin token. Its native apps
//! don't: they call the Innertube `/player` endpoint as themselves and get
//! plain, directly fetchable URLs back. This module asks as the visionOS app
//! (the Apple Vision Pro client), which needs neither a JS player nor a PO
//! token for HTTPS, DASH or HLS streams, so nothing here runs JavaScript or
//! any other script, and no external program is involved.
//!
//! Known limit: YouTube serves no streams to this client for "made for kids"
//! videos; those fail with a clear message.

use crate::captions::{CaptionLanguage, CaptionTrack, parse_captions};
use crate::error::{Error, Result};
use crate::storyboard::{Storyboard, parse_storyboard};
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// The visionOS app's identity, as YouTube expects it (InnerTube client 101).
const CLIENT_NAME: &str = "VISIONOS";
const CLIENT_ID: &str = "101";
const CLIENT_VERSION: &str = "1.02";
const DEVICE_MAKE: &str = "Apple";
const DEVICE_MODEL: &str = "RealityDevice17,1";
const OS_NAME: &str = "visionOS";
const OS_VERSION: &str = "26.5.23O471";
/// Also sent to the stream servers (as the app does), through mpv's `user-agent`.
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
/// A visitor id is good for a long time; refresh it now and then anyway.
const VISITOR_TTL: Duration = Duration::from_secs(6 * 3600);

/// What to hand the decoder.
#[derive(Clone, Debug, PartialEq)]
pub struct Streams {
    /// Video track URL (or an HLS manifest for live streams, which carries the sound too).
    pub video: String,
    /// Separate audio track URL; `None` for HLS.
    pub audio: Option<String>,
    pub is_live: bool,
    /// The video's title and channel, from the same answer (for "now playing").
    pub title: String,
    pub author: String,
    /// Height of the chosen video stream (0 for HLS, where the manifest decides).
    pub height: u32,
    /// The heights this video offers, ascending, for a quality menu.
    pub heights: Vec<u32>,
    /// e.g. "720p30 avc1 1.2 Mb/s"
    pub video_desc: String,
    /// e.g. "mp4a 129 kb/s"
    pub audio_desc: String,
    /// Milliseconds spent on the visitor id (0 when cached) and on the player request.
    pub visitor_ms: f64,
    pub player_ms: f64,
    pub captions: Vec<CaptionTrack>,
    pub caption_languages: Vec<CaptionLanguage>,
    /// Seek-bar preview sheets, when the video has them.
    pub storyboard: Option<Storyboard>,
    /// The audio languages offered (original and dubs); empty when there is only one.
    pub audio_tracks: Vec<AudioTrack>,
    /// The id in `audio_tracks` that `audio` plays.
    pub audio_track: Option<String>,
    /// Decibels of attenuation computed from YouTube's loudness metadata.
    pub normalization_gain_db: f64,
}

/// One audio language of a video: the original or a (possibly automatic) dub.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioTrack {
    /// YouTube's id, e.g. "en-US.4". Pass it to `Player::set_audio_track`.
    pub id: String,
    /// "English (US) original", "German (DE)".
    pub label: String,
    pub original: bool,
    pub auto_dubbed: bool,
    url: String,
    gain_db: f64,
}

impl Eq for AudioTrack {}

fn http() -> Result<&'static reqwest::Client> {
    static C: OnceLock<Result<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(Error::from)
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// YouTube hands every client a visitor id; requests without one are treated as suspicious.
async fn visitor_data() -> Result<(String, f64)> {
    static CACHE: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut slot = cache.lock().await;
    if let Some((v, at)) = slot.as_ref()
        && at.elapsed() < VISITOR_TTL
    {
        return Ok((v.clone(), 0.0));
    }
    let started = Instant::now();
    let _turn = crate::polite::turn().await?;
    let resp = http()?
        .get("https://www.youtube.com/sw.js_data")
        .send()
        .await?;
    crate::polite::check_status(
        resp.status(),
        resp.headers().get(reqwest::header::RETRY_AFTER),
    )?;
    let body = resp.text().await?;
    // The body is `)]}'` followed by JSON; the visitor id sits at [0][2][0][0][13].
    let json = body
        .find('[')
        .and_then(|start| body.get(start..))
        .ok_or_else(|| Error::Parse("sw.js_data: no JSON".into()))?;
    let v: Value = serde_json::from_str(json)?;
    let visitor = v
        .pointer("/0/2/0/0/13")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse("sw.js_data: no visitor id".into()))?
        .to_string();
    *slot = Some((visitor.clone(), Instant::now()));
    Ok((visitor, started.elapsed().as_secs_f64() * 1000.0))
}

/// The quality ladder offered to the viewer. Capped at 1080p: above that YouTube only serves
/// VP9/AV1, which most laptops decode in software.
pub const HEIGHTS: [u32; 6] = [144, 240, 360, 480, 720, 1080];

/// The default cap when the caller has no opinion (`NT_MAX_HEIGHT` overrides it).
pub fn default_max_height() -> u32 {
    std::env::var("NT_MAX_HEIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1080)
}

/// Find the streams for a video.
/// `max_height` caps the video stream (see [`HEIGHTS`]).
pub async fn resolve(video_id: &str, max_height: u32) -> Result<Streams> {
    resolve_with_audio(video_id, max_height, None).await
}

/// [`resolve`], playing the audio track with this id when the video has it (else the original).
pub async fn resolve_with_audio(
    video_id: &str,
    max_height: u32,
    audio_track: Option<&str>,
) -> Result<Streams> {
    let fixtures = crate::fixtures::Fixtures::from_env();
    let key = format!("POST player VISIONOS {video_id}");
    if let Some(f) = &fixtures
        && let Some(hit) = f.lookup(&key)?
    {
        return pick_with_audio(&serde_json::from_slice(&hit)?, max_height, audio_track);
    }
    let (visitor, visitor_ms) = visitor_data().await?;
    let started = Instant::now();
    let _turn = crate::polite::turn().await?;
    let body = json!({
        "context": { "client": {
            "clientName": CLIENT_NAME,
            "clientVersion": CLIENT_VERSION,
            "deviceMake": DEVICE_MAKE,
            "deviceModel": DEVICE_MODEL,
            "userAgent": USER_AGENT,
            "osName": OS_NAME,
            "osVersion": OS_VERSION,
            "hl": "en",
            "visitorData": visitor,
        }},
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let resp = http()?
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header("X-Youtube-Client-Name", CLIENT_ID)
        .header("X-Youtube-Client-Version", CLIENT_VERSION)
        .header("X-Goog-Visitor-Id", &visitor)
        .header("Origin", "https://www.youtube.com")
        .json(&body)
        .send()
        .await?;
    crate::polite::check_status(
        resp.status(),
        resp.headers().get(reqwest::header::RETRY_AFTER),
    )?;
    let v: Value = resp.json().await?;
    let player_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(f) = &fixtures {
        f.store(&key, v.to_string().as_bytes());
    }
    // Debugging aid: NT_DUMP_PLAYER=<file> saves the raw /player answer.
    if let Some(path) = std::env::var_os("NT_DUMP_PLAYER") {
        let _ = std::fs::write(path, v.to_string());
    }
    let mut streams = pick_with_audio(&v, max_height, audio_track)?;
    streams.visitor_ms = visitor_ms;
    streams.player_ms = player_ms;
    Ok(streams)
}

fn detail(v: &Value, pointer: &str) -> String {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Choose streams from a `/player` answer. Pure, so it is testable.
#[cfg(test)]
pub(crate) fn pick(v: &Value, max_height: u32) -> Result<Streams> {
    pick_with_audio(v, max_height, None)
}

pub(crate) fn pick_with_audio(
    v: &Value,
    max_height: u32,
    audio_track: Option<&str>,
) -> Result<Streams> {
    let status = v
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if status != "OK" {
        let reason = v
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .unwrap_or("This video can't be played");
        if crate::polite::check_refusal(reason) {
            return Err(Error::Network(format!(
                "{reason} (YouTube is suspicious of this connection; NativeTube has paused its requests for 30 min)"
            )));
        }
        let made_for_kids = v
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str)
            == Some("UNPLAYABLE")
            && v.pointer("/videoDetails/isMadeForKids")
                .and_then(Value::as_bool)
                == Some(true);
        return Err(Error::Player(if made_for_kids {
            "Made-for-kids videos can't be played here".to_string()
        } else {
            reason.to_string()
        }));
    }
    let data = v
        .get("streamingData")
        .ok_or_else(|| Error::Player("No streams were offered".into()))?;
    let is_live = v
        .pointer("/videoDetails/isLive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (captions, caption_languages) = parse_captions(v);
    let storyboard = parse_storyboard(v);
    let audio_config = &v["playerConfig"]["audioConfig"];
    if is_live && let Some(hls) = data.get("hlsManifestUrl").and_then(Value::as_str) {
        return Ok(Streams {
            video: hls.to_string(),
            audio: None,
            is_live: true,
            title: detail(v, "/videoDetails/title"),
            author: detail(v, "/videoDetails/author"),
            height: 0,
            heights: Vec::new(),
            video_desc: "live HLS".into(),
            audio_desc: "in HLS".into(),
            visitor_ms: 0.0,
            player_ms: 0.0,
            captions,
            caption_languages,
            storyboard,
            audio_tracks: Vec::new(),
            audio_track: None,
            normalization_gain_db: normalization_gain(audio_config, &Value::Null),
        });
    }
    let formats: Vec<&Value> = data
        .get("adaptiveFormats")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| f.get("url").and_then(Value::as_str).is_some())
                .collect()
        })
        .unwrap_or_default();
    let mime = |f: &Value| {
        f.get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let num = |f: &Value, k: &str| f.get(k).and_then(Value::as_u64).unwrap_or(0);
    let max_h = u64::from(max_height);

    // Video: H.264 first (hardware decoders everywhere), then VP9. AV1 decodes in software on
    // most machines, so it is never chosen. Highest resolution within the cap, then frame rate.
    // Video: the highest resolution within the cap, then frame rate; at equal quality H.264
    // first (hardware decoders everywhere), then VP9. AV1 decodes in software on most machines,
    // so it is never chosen.
    let playable = |f: &&&Value| {
        let m = mime(f);
        m.starts_with("video/") && (m.contains("avc1") || m.contains("vp9") || m.contains("vp09"))
    };
    let video = formats
        .iter()
        .filter(playable)
        .filter(|f| num(f, "height") <= max_h)
        .max_by_key(|f| {
            (
                num(f, "height"),
                num(f, "fps"),
                mime(f).contains("avc1"),
                num(f, "bitrate"),
            )
        })
        // Nothing under the cap (e.g. a 144p cap on a 240p-minimum video): take the smallest.
        .or_else(|| {
            formats
                .iter()
                .filter(playable)
                .min_by_key(|f| num(f, "height"))
        })
        .ok_or_else(|| Error::Player("No playable video stream".into()))?;
    let mut heights: Vec<u32> = formats
        .iter()
        .filter(playable)
        .map(|f| num(f, "height") as u32)
        .filter(|h| HEIGHTS.contains(h))
        .collect();
    heights.sort_unstable();
    heights.dedup();
    // Audio: the asked-for language or else the original, not a dub; never the
    // loudness-compressed variant when another exists; then AAC in MP4 before Opus; then the
    // highest bitrate.
    let audio_formats: Vec<&Value> = formats
        .iter()
        .copied()
        .filter(|f| mime(f).starts_with("audio/"))
        .collect();
    let audio_tracks = audio_tracks(&audio_formats, &mime, audio_config);
    let wanted = audio_track.filter(|id| audio_tracks.iter().any(|t| t.id == *id));
    let audio = best_audio(&audio_formats, wanted, &mime);
    let audio_track = audio
        .and_then(|a| a.pointer("/audioTrack/id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let codec = |m: &str| {
        m.split("codecs=\"")
            .nth(1)
            .and_then(|c| c.split(['.', '"']).next())
            .unwrap_or("?")
            .to_string()
    };
    let video_desc = format!(
        "{}p{} {} {:.1} Mb/s",
        num(video, "height"),
        num(video, "fps"),
        codec(&mime(video)),
        num(video, "bitrate") as f64 / 1e6
    );
    let audio_desc = audio
        .map(|a| format!("{} {} kb/s", codec(&mime(a)), num(a, "bitrate") / 1000))
        .unwrap_or_else(|| "none".into());
    Ok(Streams {
        video: video
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        audio: audio
            .and_then(|a| a.get("url").and_then(Value::as_str))
            .map(str::to_string),
        is_live,
        title: detail(v, "/videoDetails/title"),
        author: detail(v, "/videoDetails/author"),
        height: num(video, "height") as u32,
        heights,
        video_desc,
        audio_desc,
        captions,
        caption_languages,
        storyboard,
        audio_tracks,
        audio_track,
        normalization_gain_db: normalization_gain(audio_config, audio.unwrap_or(&Value::Null)),
        visitor_ms: 0.0,
        player_ms: 0.0,
    })
}

/// Based on Akshar Patel's yt-music-gpui normalization_gain (MIT / Apache-2.0).
fn normalization_gain(config: &Value, format: &Value) -> f64 {
    let field = |name: &str| {
        let number = |v: &Value| v[name].as_f64().filter(|n| n.is_finite());
        number(format).or_else(|| number(config))
    };
    let gain = field("loudnessTargetLkfs")
        .zip(field("trackAbsoluteLoudnessLkfs"))
        .map(|(target, absolute)| target - absolute)
        .or_else(|| field("loudnessDb").map(|db| -db))
        .unwrap_or(0.0);
    gain.clamp(-96.0, 0.0)
}

fn is_drc(f: &Value) -> bool {
    f.get("isDrc").and_then(Value::as_bool).unwrap_or(false)
}

/// The best audio format of one language: `track` (an `audioTrack.id`), or the original.
fn best_audio<'a>(
    formats: &[&'a Value],
    track: Option<&str>,
    mime: &dyn Fn(&Value) -> String,
) -> Option<&'a Value> {
    let is_original = |f: &Value| {
        f.pointer("/audioTrack/audioIsDefault")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    };
    fn id(f: &Value) -> Option<&str> {
        f.pointer("/audioTrack/id").and_then(Value::as_str)
    }
    let track = track.filter(|track| formats.iter().any(|f| id(f) == Some(*track)));
    formats.iter().copied().max_by_key(|f| {
        (
            track.map_or(is_original(f), |t| id(f) == Some(t)),
            !is_drc(f),
            mime(f).starts_with("audio/mp4"),
            f.get("bitrate").and_then(Value::as_u64).unwrap_or(0),
        )
    })
}

/// One entry per audio language, original first, then by label. Empty for a single language.
fn audio_tracks(
    formats: &[&Value],
    mime: &dyn Fn(&Value) -> String,
    audio_config: &Value,
) -> Vec<AudioTrack> {
    let mut tracks: Vec<AudioTrack> = Vec::new();
    for f in formats {
        let Some(id) = f.pointer("/audioTrack/id").and_then(Value::as_str) else {
            continue;
        };
        if tracks.iter().any(|t| t.id == id) {
            continue;
        }
        let Some(best) = best_audio(formats, Some(id), mime) else {
            continue;
        };
        tracks.push(AudioTrack {
            id: id.to_string(),
            label: f
                .pointer("/audioTrack/displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string(),
            original: f
                .pointer("/audioTrack/audioIsDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            auto_dubbed: f
                .pointer("/audioTrack/isAutoDubbed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            gain_db: normalization_gain(audio_config, best),
            url: best
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    if tracks.len() < 2 {
        return Vec::new();
    }
    tracks.sort_by(|a, b| b.original.cmp(&a.original).then(a.label.cmp(&b.label)));
    tracks
}

impl AudioTrack {
    pub(crate) fn gain_db(&self) -> f64 {
        self.gain_db
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_loudness_from_recorded_player() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/visionos-loudness.json")).unwrap();
        let streams = pick(&v, 360).unwrap();
        assert_eq!(streams.normalization_gain_db, 0.0);
        let mut loud = v.clone();
        loud["streamingData"]["adaptiveFormats"][1]["trackAbsoluteLoudnessLkfs"] = json!(-11.0);
        assert_eq!(pick(&loud, 360).unwrap().normalization_gain_db, -3.0);
        let config = json!({"loudnessDb": 5.0});
        assert_eq!(
            normalization_gain(&config, &json!({"loudnessDb": 7.0})),
            -7.0
        );
        assert_eq!(
            normalization_gain(&config, &json!({"loudnessDb": -3.0})),
            0.0
        );
        assert_eq!(
            normalization_gain(&config, &json!({"loudnessDb": 300.0})),
            -96.0
        );
    }

    #[test]
    fn picks_avc1_under_cap_and_best_audio() {
        let v = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"url": "v1080", "mimeType": "video/mp4; codecs=\"avc1.640028\"", "height": 1080, "fps": 30, "bitrate": 4000000},
                {"url": "v720", "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "height": 720, "fps": 30, "bitrate": 1500000},
                {"url": "v720av1", "mimeType": "video/mp4; codecs=\"av01.0.05M.08\"", "height": 720, "fps": 60, "bitrate": 900000},
                {"url": "a128", "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000},
                {"url": "a48", "mimeType": "audio/mp4; codecs=\"mp4a.40.5\"", "bitrate": 48000},
                {"url": "opus", "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 160000}
            ]}
        });
        let s = pick(&v, 720).unwrap();
        assert_eq!(s.video, "v720");
        assert_eq!(s.audio.as_deref(), Some("a128"));
        assert_eq!(s.video_desc, "720p30 avc1 1.5 Mb/s");
    }

    #[test]
    fn unplayable_reports_reason() {
        let v = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Sign in to confirm your age"}});
        assert_eq!(
            pick(&v, 720),
            Err(Error::Player("Sign in to confirm your age".into()))
        );
    }

    #[test]
    fn live_uses_hls() {
        let v = json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"isLive": true},
            "streamingData": {"hlsManifestUrl": "https://m/x.m3u8", "adaptiveFormats": []}
        });
        let s = pick(&v, 720).unwrap();
        assert!(s.is_live && s.audio.is_none() && s.video == "https://m/x.m3u8");
    }

    #[test]
    fn prefers_resolution_then_original_audio() {
        let v = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"url": "avc720", "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "height": 720, "fps": 30, "bitrate": 1500000},
                {"url": "vp9_1080", "mimeType": "video/webm; codecs=\"vp9\"", "height": 1080, "fps": 30, "bitrate": 2500000},
                {"url": "dub", "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 190000, "audioTrack": {"audioIsDefault": false}},
                {"url": "drc", "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 150000, "isDrc": true, "audioTrack": {"audioIsDefault": true}},
                {"url": "orig", "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000, "audioTrack": {"audioIsDefault": true}}
            ]}
        });
        let s = pick(&v, 1080).unwrap();
        assert_eq!((s.video.as_str(), s.height), ("vp9_1080", 1080));
        assert_eq!(s.audio.as_deref(), Some("orig"));
        assert_eq!(s.heights, vec![720, 1080]);
        assert_eq!(pick(&v, 720).unwrap().video, "avc720");
    }
}

#[cfg(test)]
mod audio_track_tests {
    use super::*;

    #[test]
    fn visionos_dubbed_audio_tracks() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/visionos-audio-tracks.json")).unwrap();
        let s = pick(&v, 720).unwrap();
        assert_eq!(s.audio_tracks.len(), 9);
        assert_eq!(s.audio_tracks[0].id, "en-US.4");
        assert_eq!(s.audio_tracks[0].label, "English (US) original");
        assert!(s.audio_tracks[0].original && !s.audio_tracks[0].auto_dubbed);
        assert!(
            s.audio_tracks[1..]
                .iter()
                .all(|t| t.auto_dubbed && !t.original)
        );
        assert_eq!(s.audio_track.as_deref(), Some("en-US.4"));
        // AAC 128k of the original, not a dub that happens to have a higher bitrate.
        assert_eq!(
            s.audio.as_deref(),
            Some("https://rr.googlevideo.example/140/en-US.4")
        );
        let de = s.audio_tracks.iter().find(|t| t.id == "de-DE.10").unwrap();
        assert_eq!(de.url(), "https://rr.googlevideo.example/140/de-DE.10");
        let dubbed = pick_with_audio(&v, 720, Some("de-DE.10")).unwrap();
        assert_eq!(dubbed.audio.as_deref(), Some(de.url()));
        assert_eq!(dubbed.audio_track.as_deref(), Some("de-DE.10"));
        // An unknown id falls back to the original.
        let unknown = pick_with_audio(&v, 720, Some("xx.1")).unwrap();
        assert_eq!(unknown.audio_track.as_deref(), Some("en-US.4"));
    }

    #[test]
    fn single_language_has_no_track_list() {
        let v = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"url": "v", "mimeType": "video/mp4; codecs=\"avc1\"", "height": 360},
                {"url": "a", "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 1,
                 "audioTrack": {"id": "en.4", "displayName": "English", "audioIsDefault": true}}
            ]}
        });
        let s = pick(&v, 720).unwrap();
        assert!(s.audio_tracks.is_empty());
        assert_eq!(s.audio_track.as_deref(), Some("en.4"));
    }
}
