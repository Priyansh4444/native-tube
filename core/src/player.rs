//! Video playback through libmpv, shared by every frontend.
//!
//! [`crate::resolve`] finds the stream URLs in Rust, the way YouTube's native apps
//! do. libmpv (with all scripting off: no Lua, no yt-dlp) fetches and decodes them
//! (using the GPU decoder where it can, copied back), plays the sound itself, and renders
//! each picture with its software renderer into a CPU buffer on a thread of its
//! own. Frontends read the newest frame with [`Player::frame`] and upload it
//! however their toolkit draws images. Every frontend gets the same frames from
//! the same pipeline, so a comparison measures the UI toolkit, not the decoder.

use crate::captions::{CaptionLanguage, CaptionTrack};
use crate::client::Job;
use crate::error::{Error, Result};
use crate::resolve::AudioTrack;
use crate::storyboard::Storyboard;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

#[cfg(target_os = "linux")]
#[path = "video_surface.rs"]
mod video_surface;
#[cfg(target_os = "linux")]
pub use video_surface::VideoSurface;

#[allow(non_camel_case_types)]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    pub enum mpv_handle {}
    pub enum mpv_render_context {}

    pub const FORMAT_FLAG: c_int = 3;
    pub const FORMAT_DOUBLE: c_int = 5;

    pub const EVENT_SHUTDOWN: c_int = 1;
    pub const EVENT_COMMAND_REPLY: c_int = 5;
    pub const EVENT_START_FILE: c_int = 6;
    pub const EVENT_END_FILE: c_int = 7;
    pub const EVENT_FILE_LOADED: c_int = 8;
    pub const EVENT_VIDEO_RECONFIG: c_int = 17;
    pub const EVENT_PLAYBACK_RESTART: c_int = 21;
    pub const EVENT_PROPERTY_CHANGE: c_int = 22;

    pub const END_FILE_REASON_EOF: c_int = 0;
    pub const END_FILE_REASON_ERROR: c_int = 4;

    pub const RENDER_PARAM_INVALID: c_int = 0;
    pub const RENDER_PARAM_API_TYPE: c_int = 1;
    pub const RENDER_PARAM_SW_SIZE: c_int = 17;
    pub const RENDER_PARAM_SW_FORMAT: c_int = 18;
    pub const RENDER_PARAM_SW_STRIDE: c_int = 19;
    pub const RENDER_PARAM_SW_POINTER: c_int = 20;
    pub const RENDER_UPDATE_FRAME: u64 = 1;

    #[repr(C)]
    pub struct mpv_event {
        pub event_id: c_int,
        pub error: c_int,
        pub reply_userdata: u64,
        pub data: *mut c_void,
    }

    #[repr(C)]
    pub struct mpv_event_property {
        pub name: *const c_char,
        pub format: c_int,
        pub data: *mut c_void,
    }

    #[repr(C)]
    pub struct mpv_event_start_file {
        pub playlist_entry_id: i64,
    }

    #[repr(C)]
    pub struct mpv_event_end_file {
        pub reason: c_int,
        pub error: c_int,
    }

    #[repr(C)]
    pub struct mpv_render_param {
        pub kind: c_int,
        pub data: *mut c_void,
    }

    /// libmpv is opened on first use (`load`), not linked: linking it made the dynamic loader
    /// resolve its ~200 shared libraries before `main` in every app, about 70 ms of every launch
    /// whether or not a video is ever played.
    macro_rules! mpv_api {
        ($( fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)?; )*) => {
            #[allow(non_snake_case)]
            struct Api {
                $( $name: unsafe extern "C" fn($($ty),*) $(-> $ret)?, )*
            }

            static API: std::sync::OnceLock<Result<Api, String>> = std::sync::OnceLock::new();

            /// Open libmpv and look up every function. Cheap after the first call.
            pub fn load() -> Result<(), String> {
                API.get_or_init(open).as_ref().map(|_| ()).map_err(Clone::clone)
            }

            fn open() -> Result<Api, String> {
                let lib = open_library()?;
                Ok(Api {
                    $( $name: {
                        let sym = symbol(lib, concat!(stringify!($name), "\0"))?;
                        // SAFETY: the symbol is libmpv's exported function of this name, whose C
                        // signature the declaration above mirrors (mpv/client.h, mpv/render.h).
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($ty),*) $(-> $ret)?>(sym) }
                    }, )*
                })
            }

            $(
                /// # Safety
                /// As the libmpv C function of the same name. Only called after `load` succeeded.
                pub unsafe fn $name($($arg: $ty),*) $(-> $ret)? {
                    let api = API
                        .get()
                        .and_then(|r| r.as_ref().ok())
                        .expect("libmpv used before Player::new loaded it");
                    // SAFETY: forwarded to the caller's contract.
                    unsafe { (api.$name)($($arg),*) }
                }
            )*
        };
    }

    fn open_library() -> Result<*mut c_void, String> {
        let mut names: Vec<std::path::PathBuf> = Vec::new();
        if cfg!(target_os = "macos") {
            // A double-clickable app keeps libmpv in Contents/Frameworks, next to Contents/MacOS.
            if let Ok(exe) = std::env::current_exe()
                && let Some(macos_dir) = exe.parent()
            {
                let frameworks = macos_dir.join("../Frameworks");
                names.push(frameworks.join("libmpv.2.dylib"));
                names.push(frameworks.join("libmpv.dylib"));
            }
            for name in [
                "libmpv.2.dylib",
                "libmpv.dylib",
                "/opt/homebrew/lib/libmpv.dylib",
                "/usr/local/lib/libmpv.dylib",
            ] {
                names.push(name.into());
            }
        } else {
            names.extend(["libmpv.so.2".into(), "libmpv.so".into()]);
        }
        for name in &names {
            let mut bytes = std::os::unix::ffi::OsStrExt::as_bytes(name.as_os_str()).to_vec();
            bytes.push(0);
            // SAFETY: a NUL-terminated path; the handle is never closed (libmpv stays loaded).
            let h =
                unsafe { libc::dlopen(bytes.as_ptr().cast(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if !h.is_null() {
                return Ok(h);
            }
        }
        Err(
            "libmpv is not installed (looked beside the app and for libmpv.so.2 / libmpv.dylib)"
                .to_string(),
        )
    }

    fn symbol(lib: *mut c_void, name: &str) -> Result<*mut c_void, String> {
        // SAFETY: `lib` is a live handle from dlopen and `name` ends with a NUL.
        let p = unsafe { libc::dlsym(lib, name.as_ptr().cast()) };
        if p.is_null() {
            Err(format!("libmpv lacks {}", name.trim_end_matches('\0')))
        } else {
            Ok(p)
        }
    }

    mpv_api! {
        fn mpv_create() -> *mut mpv_handle;
        fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
        fn mpv_terminate_destroy(ctx: *mut mpv_handle);
        fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
        fn mpv_command(ctx: *mut mpv_handle, args: *mut *const c_char) -> c_int;
        fn mpv_command_async(ctx: *mut mpv_handle, reply_userdata: u64, args: *mut *const c_char) -> c_int;
        fn mpv_abort_async_command(ctx: *mut mpv_handle, reply_userdata: u64);
        fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
        fn mpv_get_property_string(ctx: *mut mpv_handle, name: *const c_char) -> *mut c_char;
        fn mpv_free(data: *mut c_void);
        fn mpv_observe_property(ctx: *mut mpv_handle, id: u64, name: *const c_char, format: c_int) -> c_int;
        fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
        fn mpv_error_string(error: c_int) -> *const c_char;
        fn mpv_render_context_create(res: *mut *mut mpv_render_context, mpv: *mut mpv_handle, params: *mut mpv_render_param) -> c_int;
        fn mpv_render_context_set_update_callback(ctx: *mut mpv_render_context, callback: Option<unsafe extern "C" fn(*mut c_void)>, callback_ctx: *mut c_void);
        fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;
        fn mpv_render_context_render(ctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
        fn mpv_render_context_report_swap(ctx: *mut mpv_render_context);
        fn mpv_render_context_free(ctx: *mut mpv_render_context);
    }
}

/// The byte order frames are delivered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// R, G, B, A in increasing address (iced, Rune, `image`).
    Rgba,
    /// B, G, R, A in increasing address (GPUI's `RenderImage`).
    Bgra,
}

/// Where playback is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Nothing loaded.
    #[default]
    Idle,
    /// Resolving the stream or buffering the first picture.
    Loading,
    /// A picture is up; playing or paused (see [`PlaybackState::paused`]).
    Ready,
    /// Reached the end; the last picture stays up.
    Ended,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackState {
    pub status: Status,
    /// Seconds from the start.
    pub position: f64,
    /// Seconds; 0 until known (and for live streams).
    pub duration: f64,
    pub paused: bool,
    /// Stalled waiting for the network.
    pub buffering: bool,
    /// 0-100.
    pub volume: f64,
    pub muted: bool,
    /// The video's display size. Both dimensions are positive when known.
    pub video_size: Option<(u32, u32)>,
    /// Seconds buffered ahead of the playhead.
    pub buffered_ahead: f64,
    /// The video id or path last passed to `load`.
    pub source: Option<String>,
    /// Playback speed, 1.0 normal.
    pub speed: f64,
    /// A live stream: no duration, no seeking backwards beyond the window.
    pub is_live: bool,
    /// Height of the stream being played (0 until known, and for live streams).
    pub height: u32,
    /// Heights this video offers, ascending, for a quality menu.
    pub heights: Vec<u32>,
    pub captions: Vec<CaptionTrack>,
    pub caption_languages: Vec<CaptionLanguage>,
    /// Zero-based track in `captions`, and an optional translation language code.
    pub selected_captions: Option<(usize, Option<String>)>,
    /// Title and channel of what is playing, once the stream is found (empty for local files).
    pub title: String,
    pub channel: String,
    /// Seek-bar previews for this video, once the stream is found.
    pub storyboard: Option<Storyboard>,
    /// Audio languages (original first, then dubs); empty when the video has one.
    pub audio_tracks: Vec<AudioTrack>,
    /// The `AudioTrack::id` playing now.
    pub audio_track: Option<String>,
    /// Loudness normalization ("stable volume") is on. Kept across videos.
    pub normalize: bool,
    /// Whether this libmpv has the per-file volume-gain option.
    pub normalization_supported: bool,
    /// Attenuation from YouTube for the current track (0 for an already quiet track).
    pub normalization_gain_db: f64,
    /// The current video repeats. Turned off by every `load`.
    pub looping: bool,
}

impl PlaybackState {
    pub fn is_playing(&self) -> bool {
        self.status == Status::Ready && !self.paused
    }

    /// Width / height of the video, once both sides are known.
    pub fn aspect(&self) -> Option<f32> {
        self.video_size
            .filter(|(w, h)| *w > 0 && *h > 0)
            .map(|(w, h)| w as f32 / h as f32)
    }
}

/// One rendered picture. `pixels` is tightly packed: `width * 4` bytes a row.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Increases by one with every frame this player renders.
    pub generation: u64,
    buf: AlignedBuf,
}

impl VideoFrame {
    pub fn pixels(&self) -> &[u8] {
        &self.buf.as_slice()[..(self.width * self.height * 4) as usize]
    }

    /// The pixels as `Bytes` that keep this frame alive, without copying.
    pub fn to_bytes(self: &Arc<Self>) -> bytes::Bytes {
        bytes::Bytes::from_owner(FrameBytes(self.clone()))
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VideoFrame({}x{} {:?} #{})",
            self.width, self.height, self.format, self.generation
        )
    }
}

struct FrameBytes(Arc<VideoFrame>);

impl AsRef<[u8]> for FrameBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.pixels()
    }
}

/// A byte buffer aligned to 64 bytes, which mpv's software renderer needs for its fast path.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Block([u8; 64]);

struct AlignedBuf(Vec<Block>);

impl AlignedBuf {
    fn new(len: usize) -> AlignedBuf {
        AlignedBuf(vec![Block([0; 64]); len.div_ceil(64)])
    }
    fn len(&self) -> usize {
        self.0.len() * 64
    }
    fn as_slice(&self) -> &[u8] {
        // SAFETY: `Block` is 64 plain bytes with no padding, so the Vec's storage is `len()` initialized bytes.
        unsafe { std::slice::from_raw_parts(self.0.as_ptr().cast::<u8>(), self.len()) }
    }
    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as in `as_slice`, and `&mut self` makes the access unique.
        unsafe { std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast::<u8>(), self.len()) }
    }
}

type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct PlaybackInput {
    video: String,
    audio: Option<String>,
}

/// One `set_audio_track` waiting for its asynchronous `audio-add`.
struct PendingAudio {
    change: u64,
    ticket: u64,
    track: AudioTrack,
    /// The track that was playing, restored as the choice if the stream fails to open.
    previous: Option<String>,
    done: tokio::sync::oneshot::Sender<Result<()>>,
}

/// Marks `reply_userdata` of an asynchronous `audio-add`; the low bits are its change number.
const AUDIO_REPLY: u64 = 1 << 62;

fn audio_title(change: u64) -> String {
    format!("nt-audio-{change}")
}

struct PreparedNext {
    id: String,
    height: u32,
    task: tokio::task::JoinHandle<Result<(crate::resolve::Streams, Instant)>>,
}

struct Shared {
    state: Mutex<PlaybackState>,
    preview: bool,
    frame: Mutex<Option<Arc<VideoFrame>>>,
    generation: AtomicU64,
    waker: Mutex<Option<Waker>>,
    /// Signalled by mpv's render update callback, a resize, or shutdown.
    render_signal: (Mutex<bool>, Condvar),
    shutdown: AtomicBool,
    /// False between a `load`/`stop` and the new file being loaded, so a picture of the
    /// previous video that mpv renders late is never published as the new one's.
    accept_frames: AtomicBool,
    /// mpv reported `playback-restart` for the current file (its first frame is decoded).
    /// Cleared with `accept_frames` on every load and stop.
    pub(crate) restarted: AtomicBool,
    /// GPU-surface swaps that carried a decoded frame of the current file since its restart.
    pub(crate) presented: AtomicU64,
    /// The viewer's quality choice: `None` is Auto (from the drawn size), `Some(h)` a fixed height.
    quality: Mutex<Option<u32>>,
    /// Automatic recoveries used for the current video (expired link, dropped connection).
    recoveries: AtomicU64,
    /// Bumped by every `load` and `stop`; a resolve that finishes for an older ticket is dropped.
    load_ticket: AtomicU64,
    /// Held while issuing a command from a worker, so `Drop` can't free mpv underneath it.
    worker_cmd: Mutex<()>,
    modern_loadfile: bool,
    prepared_next: Mutex<Option<PreparedNext>>,
    input: Mutex<Option<PlaybackInput>>,
    caption_change: AtomicU64,
    caption_source: Mutex<Option<String>>,
    /// The viewer's audio language for the current video (`None`: the original). A reload after
    /// an expired link or a quality change keeps it.
    audio_choice: Mutex<Option<String>>,
    /// Bumped by every audio track switch, so only the newest one lands.
    audio_change: AtomicU64,
    /// Audio switches whose `audio-add` is still opening its stream in mpv.
    pending_audio: Mutex<Vec<PendingAudio>>,
    /// Largest picture to render, in physical pixels. Default 1280x720.
    target: Mutex<(u32, u32)>,
    stats: Mutex<RenderStats>,
    last_remembered: Mutex<Instant>,
    #[cfg(target_os = "linux")]
    surface: Mutex<Option<video_surface::Slot>>,
    /// Pinged on every state change the desktop (MPRIS) may care about.
    pub(crate) changed: tokio::sync::Notify,
}

/// Counters for the benchmark report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderStats {
    pub frames: u64,
    pub total_render_ms: f64,
    pub max_render_ms: f64,
}

impl Shared {
    /// Close the frame gate and drop the current picture, atomically with respect to the render thread.
    fn clear_frame(&self) {
        let mut slot = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        self.accept_frames.store(false, Ordering::Release);
        self.restarted.store(false, Ordering::Release);
        self.presented.store(0, Ordering::Release);
        self.lock_state().video_size = None;
        *slot = None;
        drop(slot);
        self.signal_render();
    }
    fn reset_captions(&self) {
        self.caption_change.fetch_add(1, Ordering::AcqRel);
        *self
            .caption_source
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn reset_audio(&self) {
        *self.input.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.audio_change.fetch_add(1, Ordering::AcqRel);
        *self.audio_choice.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn remember_playback(&self, force: bool) {
        if self.preview {
            return;
        }
        let mut remembered = self
            .last_remembered
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !force && remembered.elapsed() < std::time::Duration::from_secs(10) {
            return;
        }
        let state = self.lock_state();
        if state.is_live
            || !matches!(state.status, Status::Ready | Status::Ended)
            || !state.position.is_finite()
            || state.position < 0.0
        {
            return;
        }
        if let Some(id) = &state.source {
            *remembered = Instant::now();
            let checkpoint = crate::LastPlayed {
                id: id.clone(),
                position: state.position,
                title: state.title.clone(),
            };
            drop(state);
            crate::library().remember(checkpoint);
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, PlaybackState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn wake(&self) {
        self.changed.notify_one();
        let waker = self.waker.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(w) = waker {
            w();
        }
    }
    fn signal_render(&self) {
        let (lock, cv) = &self.render_signal;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_one();
    }
}

struct Handle(*mut ffi::mpv_handle);
// SAFETY: the libmpv client API is thread-safe; a handle may be used from any thread.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

struct RenderCtx(*mut ffi::mpv_render_context);
// SAFETY: the render context is used from the render thread only after creation.
unsafe impl Send for RenderCtx {}

/// What the desktop may do to a player (see `mpris.rs`). Safe after the player is dropped: every
/// call takes the same guard `Drop` takes and does nothing once the player is gone.
#[derive(Clone)]
pub(crate) struct Remote {
    mpv: Arc<Handle>,
    shared: Arc<Shared>,
}

impl Remote {
    fn with(&self, f: impl FnOnce(&Handle)) {
        let _guard = self
            .shared
            .worker_cmd
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !self.shared.shutdown.load(Ordering::Acquire) {
            f(&self.mpv);
        }
    }
    pub(crate) fn is_gone(&self) -> bool {
        self.shared.shutdown.load(Ordering::Acquire)
    }
    pub(crate) fn state(&self) -> PlaybackState {
        self.shared.lock_state().clone()
    }
    pub(crate) async fn changed(&self) {
        self.shared.changed.notified().await;
    }
    pub(crate) fn set_paused(&self, paused: bool) {
        self.with(|m| set_flag_raw(m, "pause", paused));
    }
    pub(crate) fn seek_to(&self, seconds: f64) {
        self.with(|m| {
            let _ = command(
                m,
                &[
                    "seek",
                    &format!("{:.3}", seconds.max(0.0)),
                    "absolute+exact",
                ],
            );
        });
    }
    pub(crate) fn seek_by(&self, seconds: f64) {
        self.with(|m| {
            let _ = command(m, &["seek", &format!("{seconds:.3}"), "relative+exact"]);
        });
    }
}

fn set_flag_raw(mpv: &Handle, name: &str, on: bool) {
    let n = cstr(name);
    let mut v: c_int = on.into();
    // SAFETY: valid handle (callers hold the worker guard or own the Player); FLAG takes an int.
    unsafe {
        ffi::mpv_set_property(mpv.0, n.as_ptr(), ffi::FORMAT_FLAG, (&raw mut v).cast());
    }
}

/// A video player. Dropping it stops playback and frees mpv.
pub struct Player {
    mpv: Arc<Handle>,
    shared: Arc<Shared>,
    format: PixelFormat,
    render_thread: Option<JoinHandle<()>>,
    event_thread: Option<JoinHandle<()>>,
}

const OBSERVED: &[(&str, c_int)] = &[
    ("time-pos", ffi::FORMAT_DOUBLE),
    ("duration", ffi::FORMAT_DOUBLE),
    ("pause", ffi::FORMAT_FLAG),
    ("paused-for-cache", ffi::FORMAT_FLAG),
    ("volume", ffi::FORMAT_DOUBLE),
    ("mute", ffi::FORMAT_FLAG),
    ("demuxer-cache-duration", ffi::FORMAT_DOUBLE),
    ("eof-reached", ffi::FORMAT_FLAG),
    ("speed", ffi::FORMAT_DOUBLE),
];

fn caption_fonts() -> &'static str {
    static FONTS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FONTS.get_or_init(|| {
        let dir = crate::data_dir().join("subtitle-fonts");
        let bytes = include_bytes!("../../assets/fonts/Roboto-Regular.ttf");
        let install = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("Roboto-Regular.ttf");
            if std::fs::read(&path).ok().as_deref() != Some(bytes) {
                let tmp = dir.join(format!("Roboto-{}.tmp", std::process::id()));
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(tmp, path)?;
            }
            Ok(())
        };
        if let Err(e) = install() {
            eprintln!("nt-core: could not install the bundled subtitle font: {e}");
        }
        dir.to_string_lossy().into_owned()
    })
}

fn force_muted() -> bool {
    std::env::var("NT_MUTE").is_ok_and(|v| v != "0")
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn mpv_err(code: c_int) -> String {
    // SAFETY: mpv_error_string returns a static string for any code.
    unsafe { CStr::from_ptr(ffi::mpv_error_string(code)) }
        .to_string_lossy()
        .into_owned()
}

unsafe extern "C" fn on_render_update(ctx: *mut c_void) {
    // SAFETY: `ctx` is the `Arc<Shared>` pointer registered in `Player::new`; it outlives the render context.
    let shared = unsafe { &*(ctx as *const Shared) };
    shared.signal_render();
}

impl Player {
    /// Start mpv. Frames come in `format`; `waker` runs (on a player thread) whenever a new
    /// frame or a state change is ready, so the UI can schedule a redraw.
    pub fn new(format: PixelFormat, waker: impl Fn() + Send + Sync + 'static) -> Result<Player> {
        Self::new_mode(format, waker, false)
    }

    /// A muted software player for hover previews, capped at 360p.
    pub fn new_preview(
        format: PixelFormat,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> Result<Player> {
        Self::new_mode(format, waker, true)
    }

    fn new_mode(
        format: PixelFormat,
        waker: impl Fn() + Send + Sync + 'static,
        preview: bool,
    ) -> Result<Player> {
        ffi::load().map_err(Error::Player)?;
        // SAFETY: plain constructor; null on failure.
        let raw = unsafe { ffi::mpv_create() };
        if raw.is_null() {
            return Err(Error::Player("mpv_create failed".into()));
        }
        let mpv = Arc::new(Handle(raw));
        let volume = crate::library::library().volume();
        let fonts = caption_fonts();
        let options = [
            ("vo", "libmpv".to_string()),
            ("sub-auto", "no".into()),
            ("sid", "no".into()),
            ("sub-font", "Roboto".into()),
            ("sub-fonts-dir", fonts.into()),
            ("sub-color", "#FFFFFF".into()),
            ("sub-back-color", "#B3000000".into()),
            ("sub-border-style", "background-box".into()),
            ("sub-align-x", "center".into()),
            ("sub-align-y", "bottom".into()),
            ("sub-margin-y", "44".into()),
            // VA-API first: on hybrid laptops it decodes on the GPU that drives the display; "auto"
            // picked NVDEC here, which wakes the discrete GPU and took ~950 ms to start instead
            // of ~210 ms. Falls back to whatever else works.
            ("hwdec", "vaapi-copy,auto-copy".to_string()),
            // The GPU surface (VideoSurface) needs only the VA-API interop. mpv's default probes
            // every interop when the OpenGL context is created, and CUDA's probe woke the
            // discrete GPU: attach took 1129 ms instead of 79 ms, and 27 MB more memory.
            ("gpu-hwdec-interop", "vaapi".to_string()),
            // No scripting of any kind: no built-in Lua scripts (ytdl_hook, osc, ...), no yt-dlp.
            ("ytdl", "no".to_string()),
            ("load-scripts", "no".to_string()),
            ("load-auto-profiles", "no".to_string()),
            ("load-commands", "no".to_string()),
            ("load-console", "no".to_string()),
            ("load-context-menu", "no".to_string()),
            ("load-positioning", "no".to_string()),
            ("load-select", "no".to_string()),
            ("load-stats-overlay", "no".to_string()),
            ("osc", "no".to_string()),
            ("input-builtin-bindings", "no".to_string()),
            // Buffer about as far ahead as YouTube's own player does, not mpv's default 1000 s.
            ("cache-secs", "60".to_string()),
            // NT_MUTE=1 starts muted (all test and benchmark runs set it).
            ("mute", if force_muted() { "yes" } else { "no" }.to_string()),
            // Stream requests carry the same identity the streams were issued to.
            ("user-agent", crate::resolve::USER_AGENT.to_string()),
            // Fetch in bounded chunks, as YouTube's own players do: googlevideo trickles an
            // open-ended `Range: bytes=0-` (~12 KB/s) but serves bounded ranges at full speed.
            // Measured on a 14 h video: first picture 16.6 s -> 0.7 s.
            (
                "stream-lavf-o",
                "request_size=10485760,initial_request_size=1048576".to_string(),
            ),
            ("keep-open", "yes".to_string()),
            ("idle", "yes".to_string()),
            ("cache", "yes".to_string()),
            ("demuxer-max-bytes", "64MiB".to_string()),
            ("demuxer-max-back-bytes", "16MiB".to_string()),
            ("audio-client-name", "NativeTube".to_string()),
            ("input-default-bindings", "no".to_string()),
            ("terminal", "no".to_string()),
            // Signal each picture when it is due rather than blocking the render call until then
            // (render.h, MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME). The UI shows it on its next frame.
            ("video-timing-offset", "0".to_string()),
            ("volume", format!("{volume}")),
        ];
        // `NT_MPV_OPTS="name=value,name=value"` adds or overrides options, for experiments.
        let extra: Vec<(String, String)> = std::env::var("NT_MPV_OPTS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            })
            .collect();
        let options = options
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .chain(extra)
            .chain(
                [
                    ("mute", "yes"),
                    ("aid", "no"),
                    ("cache-secs", "3"),
                    ("demuxer-max-bytes", "8MiB"),
                    ("demuxer-max-back-bytes", "1MiB"),
                ]
                .into_iter()
                .filter(|_| preview)
                .map(|(k, v)| (k.to_owned(), v.to_owned())),
            );
        for (name, value) in options {
            let (n, v) = (cstr(&name), cstr(&value));
            // SAFETY: valid handle and NUL-terminated strings.
            unsafe { ffi::mpv_set_option_string(raw, n.as_ptr(), v.as_ptr()) };
        }
        // SAFETY: valid handle, not yet initialized.
        let rc = unsafe { ffi::mpv_initialize(raw) };
        if rc < 0 {
            // SAFETY: the handle is ours and not used after this.
            unsafe { ffi::mpv_terminate_destroy(raw) };
            return Err(Error::Player(format!("mpv_initialize: {}", mpv_err(rc))));
        }
        for (i, (name, fmt)) in OBSERVED.iter().enumerate() {
            let n = cstr(name);
            // SAFETY: valid handle and name.
            unsafe { ffi::mpv_observe_property(raw, i as u64, n.as_ptr(), *fmt) };
        }

        let modern_loadfile =
            get_string(&mpv, "mpv-version").is_some_and(|s| supports_modern_loadfile(&s));
        let normalization_supported =
            modern_loadfile && get_string(&mpv, "option-info/volume-gain/name").is_some();
        let shared = Arc::new(Shared {
            preview,
            state: Mutex::new(PlaybackState {
                speed: 1.0,
                muted: preview || force_muted(),
                normalize: true,
                normalization_supported,
                volume,
                ..Default::default()
            }),
            frame: Mutex::new(None),
            generation: AtomicU64::new(0),
            waker: Mutex::new(Some(Arc::new(waker))),
            render_signal: (Mutex::new(false), Condvar::new()),
            shutdown: AtomicBool::new(false),
            accept_frames: AtomicBool::new(false),
            restarted: AtomicBool::new(false),
            presented: AtomicU64::new(0),
            quality: Mutex::new(None),
            recoveries: AtomicU64::new(0),
            load_ticket: AtomicU64::new(0),
            worker_cmd: Mutex::new(()),
            modern_loadfile,
            prepared_next: Mutex::new(None),
            input: Mutex::new(None),
            caption_change: AtomicU64::new(0),
            caption_source: Mutex::new(None),
            audio_choice: Mutex::new(None),
            audio_change: AtomicU64::new(0),
            pending_audio: Mutex::new(Vec::new()),
            target: Mutex::new(if preview { (640, 360) } else { (1280, 720) }),
            stats: Mutex::new(RenderStats::default()),
            last_remembered: Mutex::new(Instant::now()),
            #[cfg(target_os = "linux")]
            surface: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
        });

        let ctx = match software_context(&mpv, &shared) {
            Ok(ctx) => ctx,
            Err(e) => {
                // SAFETY: the unshared handle has no render context or worker.
                unsafe { ffi::mpv_terminate_destroy(raw) };
                return Err(e);
            }
        };
        let render_thread = {
            let (shared, mpv) = (shared.clone(), mpv.clone());
            std::thread::Builder::new()
                .name("nt-player-render".into())
                .spawn(move || render_loop(ctx, mpv, shared, format))
                .map_err(|e| Error::Player(e.to_string()))?
        };
        let event_thread = {
            let (shared, mpv) = (shared.clone(), mpv.clone());
            std::thread::Builder::new()
                .name("nt-player-events".into())
                .spawn(move || event_loop(mpv, shared))
                .map_err(|e| Error::Player(e.to_string()))?
        };
        #[cfg(target_os = "linux")]
        if !preview {
            crate::mpris::start(Remote {
                mpv: mpv.clone(),
                shared: shared.clone(),
            });
        }
        Ok(Player {
            mpv,
            shared,
            format,
            render_thread: Some(render_thread),
            event_thread: Some(event_thread),
        })
    }

    /// Replace the waker (for toolkits that only get a waker after the player exists).
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *self.shared.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(waker));
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        command(&self.mpv, args)
    }

    fn set_flag(&self, name: &str, on: bool) {
        let n = cstr(name);
        let mut v: c_int = on.into();
        // SAFETY: valid handle; FLAG takes a pointer to an int.
        unsafe {
            ffi::mpv_set_property(
                self.mpv.0,
                n.as_ptr(),
                ffi::FORMAT_FLAG,
                (&raw mut v).cast(),
            )
        };
    }

    fn set_double(&self, name: &str, value: f64) {
        let n = cstr(name);
        let mut v = value;
        // SAFETY: valid handle; DOUBLE takes a pointer to a double.
        unsafe {
            ffi::mpv_set_property(
                self.mpv.0,
                n.as_ptr(),
                ffi::FORMAT_DOUBLE,
                (&raw mut v).cast(),
            )
        };
    }

    /// Resolve only the natural successor. Calling with None drops the candidate.
    /// A matching load joins the pending work or reuses the prepared streams.
    pub fn prepare_next(&self, video_id: Option<&str>) {
        if self.shared.preview {
            return;
        }
        let height = wanted_height(&self.shared);
        let mut slot = self
            .shared
            .prepared_next
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(current) = slot.as_ref()
            && Some(current.id.as_str()) == video_id
            && current.height == height
        {
            return;
        }
        if let Some(old) = slot.take() {
            old.task.abort();
        }
        if let Some(video_id) = video_id.filter(|id| !id.is_empty())
            && std::env::var("NT_VIDEO_OVERRIDE").map_or(true, |p| p.is_empty())
        {
            let id = video_id.to_owned();
            let task = crate::runtime().spawn(async move {
                let streams = crate::resolve::resolve(&id, height).await?;
                Ok((streams, Instant::now()))
            });
            *slot = Some(PreparedNext {
                id: video_id.to_owned(),
                height,
                task,
            });
        }
    }

    /// Open a YouTube video by id and start playing. Returns at once: the streams are found on a
    /// core worker ([`crate::resolve`]) while the state says `Loading`; a failure becomes
    /// `Status::Failed`. `NT_VIDEO_OVERRIDE=<file>` plays that file instead (benchmarks use it to
    /// take the network out of the measurement).
    pub fn load(&self, video_id: &str) -> Result<()> {
        self.load_at(video_id, 0.0)
    }

    /// Open a video at an offset in seconds. Live streams start at the live position.
    pub fn load_at(&self, video_id: &str, start_secs: f64) -> Result<()> {
        let start = (start_secs.is_finite() && start_secs > 0.0).then_some(start_secs);
        let guard = self
            .shared
            .worker_cmd
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ticket = self.shared.load_ticket.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.remember_playback(true);
        self.shared.reset_captions();
        self.shared.reset_audio();
        self.shared.clear_frame();
        {
            let mut s = self.shared.lock_state();
            let (volume, muted, normalize) = (s.volume, s.muted, s.normalize);
            *s = PlaybackState {
                status: Status::Loading,
                volume,
                muted,
                normalize,
                normalization_supported: s.normalization_supported,
                source: Some(video_id.to_string()),
                ..Default::default()
            };
        }
        self.set_flag("pause", false);
        let _ = self.command(&["set", "loop-file", "no"]);
        // Unload the old file before resolving; a switch can reuse the active input.
        let _ = self.command(&["stop"]);
        let local = std::env::var("NT_VIDEO_OVERRIDE")
            .ok()
            .filter(|path| !path.is_empty())
            .map(|video| {
                open_input(
                    &self.mpv,
                    &self.shared,
                    PlaybackInput { video, audio: None },
                    start,
                )
            });
        drop(guard);
        self.shared.wake();
        if let Some(result) = local {
            return result;
        }
        self.shared.recoveries.store(0, Ordering::Release);
        begin_load(&self.mpv, &self.shared, video_id, start, ticket, true);
        Ok(())
    }

    /// For app exit: stop playback, then write the resume point and the rest of the library to
    /// disk before returning. Call it from the UI thread as the app quits; it blocks only for one
    /// small file write. Needed when the player lives in a static, whose `Drop` never runs.
    pub fn shutdown(&self) {
        self.stop();
        if !self.shared.preview {
            crate::library().flush();
        }
    }

    /// Stop and unload; frees the decoder and the stream.
    pub fn stop(&self) {
        self.prepare_next(None);
        let guard = self
            .shared
            .worker_cmd
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared.remember_playback(true);
        self.shared.reset_captions();
        self.shared.reset_audio();
        self.shared.load_ticket.fetch_add(1, Ordering::AcqRel);
        self.shared.clear_frame();
        let _ = self.command(&["stop"]);
        let _ = self.command(&["set", "loop-file", "no"]);
        {
            // Volume, mute and normalization are mpv's settings, not the file's: they survive a stop.
            let mut st = self.shared.lock_state();
            let (volume, muted, normalize) = (st.volume, st.muted, st.normalize);
            *st = PlaybackState {
                volume,
                muted,
                normalize,
                normalization_supported: st.normalization_supported,
                ..Default::default()
            };
        }
        drop(guard);
        self.shared.wake();
    }

    pub fn play(&self) {
        if self.state().status == Status::Ended {
            self.seek(0.0);
        }
        self.set_flag("pause", false);
    }

    pub fn pause(&self) {
        self.set_flag("pause", true);
    }

    pub fn toggle_pause(&self) {
        if self.state().paused || self.state().status == Status::Ended {
            self.play()
        } else {
            self.pause()
        }
    }

    /// Seek to an absolute position in seconds.
    pub fn seek(&self, seconds: f64) {
        let _ = self.command(&[
            "seek",
            &format!("{:.3}", seconds.max(0.0)),
            "absolute+exact",
        ]);
    }

    /// Seek by a relative amount in seconds.
    pub fn seek_by(&self, seconds: f64) {
        let _ = self.command(&["seek", &format!("{seconds:.3}"), "relative+exact"]);
    }

    /// Playback speed, 0.25 to 2.0.
    pub fn set_speed(&self, speed: f64) {
        self.set_double("speed", speed.clamp(0.25, 2.0));
    }

    /// The viewer's quality choice: `None` for Auto (picked from the size the video is drawn at),
    /// or a height from [`PlaybackState::heights`]. Changing it re-opens the current video at the
    /// same position.
    pub fn set_quality(&self, height: Option<u32>) {
        {
            let mut q = self
                .shared
                .quality
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *q == height {
                return;
            }
            *q = height;
        }
        let st = self.state();
        if let Some(id) = st.source.filter(|_| !matches!(st.status, Status::Idle))
            && std::env::var("NT_VIDEO_OVERRIDE").map_or(true, |p| p.is_empty())
        {
            let ticket = self.shared.load_ticket.fetch_add(1, Ordering::AcqRel) + 1;
            begin_load(
                &self.mpv,
                &self.shared,
                &id,
                Some(st.position),
                ticket,
                false,
            );
        }
    }

    /// Load a timed-text track into the rendered frame, or turn captions off.
    /// Pass an index and optional translation language code from the current state.
    pub fn set_captions(&self, selection: Option<(usize, Option<String>)>) -> Job<()> {
        let st = self.state();
        if !matches!(st.status, Status::Ready | Status::Ended) {
            return Job::ready(Err(Error::Player("Wait for the video to load".into())));
        }
        let request = selection
            .as_ref()
            .map(|(index, language)| {
                let track = st
                    .captions
                    .get(*index)
                    .ok_or_else(|| Error::Player("Unknown caption track".into()))?;
                if let Some(code) = language
                    && (!track.translatable
                        || !st.caption_languages.iter().any(|l| &l.code == code))
                {
                    return Err(Error::Player("Unavailable caption translation".into()));
                }
                track.timedtext_url(language.as_deref())
            })
            .transpose();
        let url = match request {
            Ok(url) => url,
            Err(e) => return Job::ready(Err(e)),
        };
        let change = self.shared.caption_change.fetch_add(1, Ordering::AcqRel) + 1;
        let ticket = self.shared.load_ticket.load(Ordering::Acquire);
        let (shared, mpv) = (self.shared.clone(), self.mpv.clone());
        Job::spawn(async move {
            let input = match url {
                Some(url) => Some(crate::captions::caption_input(&url).await?),
                None => None,
            };
            let guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
            if shared.shutdown.load(Ordering::Acquire)
                || shared.load_ticket.load(Ordering::Acquire) != ticket
                || shared.caption_change.load(Ordering::Acquire) != change
            {
                return Err(Error::Cancelled);
            }
            let mut source = shared
                .caption_source
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if source.is_some() {
                // Removal without an ID removes the currently selected external track.
                let _ = command(&mpv, &["sub-remove"]);
            }
            let result = input
                .as_ref()
                .map_or(Ok(()), |input| command(&mpv, &["sub-add", input, "select"]));
            *source = input.filter(|_| result.is_ok());
            shared.lock_state().selected_captions = selection.filter(|_| result.is_ok());
            drop(source);
            drop(guard);
            shared.wake();
            result
        })
    }

    pub fn quality(&self) -> Option<u32> {
        *self
            .shared
            .quality
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Play another audio language of the current video (an id from
    /// [`PlaybackState::audio_tracks`]), keeping the position. The video keeps playing while the
    /// new track opens on a core worker; the sound switches when it is ready (typically well under
    /// a second). The choice holds until the next `load`, including across a quality change.
    pub fn set_audio_track(&self, id: &str) -> Job<()> {
        if self.shared.preview {
            return Job::ready(Err(Error::Player("Preview playback has no audio".into())));
        }
        let _guard = self
            .shared
            .worker_cmd
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let st = self.state();
        if !matches!(st.status, Status::Ready | Status::Ended) {
            return Job::ready(Err(Error::Player("Wait for the video to load".into())));
        }
        let Some(track) = st.audio_tracks.iter().find(|t| t.id == id).cloned() else {
            return Job::ready(Err(Error::Player("Unknown audio track".into())));
        };
        // Record the choice first: a reload (quality change, expired link) that starts while the
        // stream opens then already asks for this track.
        let change = self.shared.audio_change.fetch_add(1, Ordering::AcqRel) + 1;
        *self
            .shared
            .audio_choice
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(track.id.clone());
        let mut pending = self
            .shared
            .pending_audio
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for older in pending.iter() {
            // SAFETY: the guard keeps the handle live. Aborting sends a signal and returns.
            unsafe { ffi::mpv_abort_async_command(self.mpv.0, AUDIO_REPLY | older.change) };
        }
        let previous = st.audio_track;
        if previous.as_deref() == Some(id) {
            return Job::ready(Ok(()));
        }
        let (done, rx) = tokio::sync::oneshot::channel();
        let title = audio_title(change);
        let url = track.url().to_owned();
        pending.push(PendingAudio {
            change,
            ticket: self.shared.load_ticket.load(Ordering::Acquire),
            track,
            previous: previous.clone(),
            done,
        });
        // Open off-thread without selecting: a reply from a superseded request must never
        // change what is playing, even if mpv finished opening it before the abort arrived.
        let issued = command_async(
            &self.mpv,
            AUDIO_REPLY | change,
            &["audio-add", &url, "auto", &title],
        );
        if let Err(e) = issued {
            pending.retain(|p| p.change != change);
            *self
                .shared
                .audio_choice
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = previous;
            return Job::ready(Err(e));
        }
        Job::from_receiver(rx)
    }

    /// Apply YouTube's per-track attenuation, independently from the user's volume.
    /// On by default. Playback uses unity gain on versions of mpv before 0.38.
    pub fn set_normalize(&self, on: bool) {
        let mut state = self.shared.lock_state();
        let gain = state.normalization_supported.then_some(if on {
            state.normalization_gain_db
        } else {
            0.0
        });
        state.normalize = on;
        drop(state);
        if let Some(gain) = gain {
            set_volume_gain(&self.mpv, gain);
        }
        self.shared.wake();
    }

    /// Repeat the current video when it ends. Every `load` turns it off again.
    pub fn set_loop(&self, on: bool) {
        if self
            .command(&["set", "loop-file", if on { "inf" } else { "no" }])
            .is_ok()
        {
            self.shared.lock_state().looping = on;
            self.shared.wake();
        }
    }

    /// Where captions sit, as mpv's `sub-pos`: the caption block's vertical position in percent
    /// of the picture's height (100 = the normal bottom position, smaller = higher). Frontends
    /// lift captions above their control bar while it shows. Immediate, no I/O, kept across
    /// loads; values are clamped to 0-150.
    pub fn set_caption_position(&self, percent: f64) {
        if percent.is_finite() {
            self.set_double("sub-pos", percent.clamp(0.0, 150.0));
        }
    }

    /// 0-100. Remembered across launches.
    pub fn set_volume(&self, volume: f64) {
        let v = volume.clamp(0.0, 100.0);
        self.set_double("volume", v);
        if !self.shared.preview {
            crate::library::library().set_volume(v);
        }
    }

    /// With `NT_MUTE` set (every test and benchmark run) the player cannot be unmuted.
    pub fn set_muted(&self, muted: bool) {
        self.set_flag("mute", self.shared.preview || muted || force_muted());
    }

    /// The largest picture worth rendering, in physical pixels (the size the video is drawn at).
    /// Rendering is capped at the video's own size, so a big box doesn't upscale on the CPU.
    pub fn set_target_size(&self, width: u32, height: u32) {
        let limit = if self.shared.preview {
            (640, 360)
        } else {
            (3840, 2160)
        };
        let new = (width.clamp(64, limit.0), height.clamp(36, limit.1));
        let mut t = self.shared.target.lock().unwrap_or_else(|e| e.into_inner());
        if *t != new {
            *t = new;
            drop(t);
            self.shared.signal_render();
        }
    }

    pub fn state(&self) -> PlaybackState {
        self.shared.lock_state().clone()
    }

    /// The newest rendered frame, if any.
    pub fn frame(&self) -> Option<Arc<VideoFrame>> {
        self.shared
            .frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Changes whenever a new frame is published; cheap to compare every UI frame.
    pub fn frame_generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// GPU surface only: frames of the current file presented on the surface since mpv's
    /// `playback-restart` for it (the first decoded picture). 0 after every `load`/`stop`, and
    /// while the surface shows nothing or only the black render it maps with. Cut the hole when
    /// this is above 0.
    pub fn presented_frames(&self) -> u64 {
        self.shared.presented.load(Ordering::Acquire)
    }

    pub fn render_stats(&self) -> RenderStats {
        *self.shared.stats.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Pending audio switches end as cancelled rather than waiting for a reply that won't come.
        self.shared
            .pending_audio
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.prepare_next(None);
        self.shared.remember_playback(true);
        if !self.shared.preview {
            crate::library().flush();
        }
        {
            // Wait out a worker that is mid-command, then make sure no other one starts.
            let _guard = self
                .shared
                .worker_cmd
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.shared.shutdown.store(true, Ordering::Release);
        }
        self.shared.changed.notify_one();
        self.shared.signal_render();
        if let Some(t) = self.render_thread.take() {
            let _ = t.join();
        }
        // The render context is gone; now mpv can go. This wakes the event thread with SHUTDOWN.
        // SAFETY: no render context remains and no other thread issues calls after this but `mpv_wait_event`,
        // which returns SHUTDOWN; the event thread then exits without touching the handle again.
        let _ = self.command(&["quit"]);
        if let Some(t) = self.event_thread.take() {
            let _ = t.join();
        }
        // SAFETY: both threads have exited; this is the last use of the handle.
        unsafe { ffi::mpv_terminate_destroy(self.mpv.0) };
    }
}

fn supports_modern_loadfile(version: &str) -> bool {
    // libmpv reports e.g. "mpv v0.42.0" (release builds) or "mpv 0.38.0-12-gabc" (git builds).
    let version = version.trim_start_matches("mpv ").trim_start_matches('v');
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };
    major
        .parse::<u32>()
        .ok()
        .zip(
            minor
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|minor| minor.parse::<u32>().ok()),
        )
        .is_some_and(|v| v >= (0, 38))
}

fn set_volume_gain(mpv: &Handle, gain: f64) {
    let _ = command(mpv, &["set", "volume-gain", &format!("{gain:.3}")]);
}

fn load_file(mpv: &Handle, modern: bool, url: &str, options: &str) -> Result<()> {
    let mut args = vec!["loadfile", url, "replace"];
    if !options.is_empty() {
        if modern {
            args.push("-1");
        }
        args.push(options);
    }
    command(mpv, &args)
}

/// A property as mpv formats it, e.g. "1" for `aid`; `None` when it has no value.
fn get_string(mpv: &Handle, name: &str) -> Option<String> {
    let n = cstr(name);
    // SAFETY: valid handle (callers hold the worker guard or own the Player) and name.
    let p = unsafe { ffi::mpv_get_property_string(mpv.0, n.as_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: mpv returned a NUL-terminated string it allocated; we copy it, then free it.
    let value = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    // SAFETY: as above; freed exactly once.
    unsafe { ffi::mpv_free(p.cast()) };
    Some(value)
}

fn current_playlist_entry(mpv: &Handle) -> Option<i64> {
    let pos = get_string(mpv, "playlist-pos")?.parse::<u32>().ok()?;
    get_string(mpv, &format!("playlist/{pos}/id"))?.parse().ok()
}

fn video_display_size(mpv: &Handle) -> Option<(u32, u32)> {
    #[derive(serde::Deserialize)]
    struct OutputParams {
        dw: u32,
        dh: u32,
    }

    // mpv formats this map as JSON. Read both post-filter dimensions in one snapshot.
    let params = get_string(mpv, "video-out-params")?;
    let OutputParams { dw, dh } = serde_json::from_str(&params).ok()?;
    (dw > 0 && dh > 0).then_some((dw, dh))
}

fn open_input(
    mpv: &Handle,
    shared: &Shared,
    input: PlaybackInput,
    start: Option<f64>,
) -> Result<()> {
    let (gain, live, paused) = {
        let st = shared.lock_state();
        (
            st.normalization_supported.then_some(if st.normalize {
                st.normalization_gain_db
            } else {
                0.0
            }),
            st.is_live,
            st.paused,
        )
    };
    // %N% quotes a value of N bytes, so commas and '=' in a URL are safe.
    let mut options = Vec::new();
    if let Some(audio) = &input.audio {
        options.push(format!("audio-file=%{}%{}", audio.len(), audio));
    }
    if let Some(t) = start.filter(|t| *t > 0.0 && !live) {
        options.push(format!("start={t:.3}"));
    }
    if paused {
        options.push("pause=yes".to_owned());
    }
    if let Some(gain) = gain {
        options.push(format!("volume-gain={gain:.3}"));
    }
    load_file(
        mpv,
        shared.modern_loadfile,
        &input.video,
        &options.join(","),
    )?;
    shared.clear_frame();
    *shared.input.lock().unwrap_or_else(|e| e.into_inner()) = Some(input);
    Ok(())
}

/// Issue a command that completes later with an `EVENT_COMMAND_REPLY` carrying `reply`.
fn command_async(mpv: &Handle, reply: u64, args: &[&str]) -> Result<()> {
    let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
    let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // SAFETY: as in `command`; mpv copies the arguments before returning.
    let rc = unsafe { ffi::mpv_command_async(mpv.0, reply, ptrs.as_mut_ptr()) };
    if rc < 0 {
        Err(Error::Player(format!(
            "{}: {}",
            args.first().copied().unwrap_or_default(),
            mpv_err(rc)
        )))
    } else {
        Ok(())
    }
}

/// mpv's audio tracks as (id, title).
fn mpv_audio_tracks(mpv: &Handle) -> Vec<(String, String)> {
    let count = get_string(mpv, "track-list/count")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    (0..count)
        .filter(|i| get_string(mpv, &format!("track-list/{i}/type")).as_deref() == Some("audio"))
        .filter_map(|i| {
            Some((
                get_string(mpv, &format!("track-list/{i}/id"))?,
                get_string(mpv, &format!("track-list/{i}/title")).unwrap_or_default(),
            ))
        })
        .collect()
}

/// An `audio-add` finished. Keep exactly one audio track: the newest request's, if it is
/// still wanted for the file that is still loaded.
fn finish_audio_switch(mpv: &Handle, shared: &Shared, change: u64, error: c_int) {
    let pending = {
        let mut list = shared
            .pending_audio
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(at) = list.iter().position(|p| p.change == change) else {
            return;
        };
        list.swap_remove(at)
    };
    let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
    let title = audio_title(change);
    let result = if shared.shutdown.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else if shared.load_ticket.load(Ordering::Acquire) != pending.ticket {
        // Another file replaced this one (a reload keeps the recorded choice).
        let chosen = shared
            .audio_choice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(pending.track.id.as_str());
        if chosen {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    } else if shared.audio_change.load(Ordering::Acquire) != change {
        // mpv may have finished adding this track before the abort landed.
        if error >= 0 {
            for (id, t) in mpv_audio_tracks(mpv) {
                if t == title {
                    let _ = command(mpv, &["audio-remove", &id]);
                }
            }
        }
        Err(Error::Cancelled)
    } else {
        let selected = if error < 0 {
            Err(Error::Player(format!(
                "Couldn't open the audio track ({})",
                mpv_err(error)
            )))
        } else {
            select_added_audio(mpv, &title)
        };
        if let Err(e) = selected {
            *shared
                .audio_choice
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = pending.previous;
            let _ = pending.done.send(Err(e));
            return;
        }
        if let Some(input) = shared
            .input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            input.audio = Some(pending.track.url().to_owned());
        }
        let mut state = shared.lock_state();
        state.normalization_gain_db = pending.track.gain_db();
        state.audio_track = Some(pending.track.id.clone());
        let gain = (state.normalization_supported && state.normalize)
            .then_some(state.normalization_gain_db);
        drop(state);
        if let Some(gain) = gain {
            set_volume_gain(mpv, gain);
        }
        Ok(())
    };
    let _ = pending.done.send(result);
}

fn select_added_audio(mpv: &Handle, title: &str) -> Result<()> {
    let tracks = mpv_audio_tracks(mpv);
    let Some((wanted, _)) = tracks.iter().find(|(_, name)| name == title) else {
        return Err(Error::Player("The added stream has no audio track".into()));
    };
    command(mpv, &["set", "aid", wanted])?;
    for (id, _) in tracks.iter().filter(|(id, _)| id != wanted) {
        let _ = command(mpv, &["audio-remove", id]);
    }
    Ok(())
}

fn command(mpv: &Handle, args: &[&str]) -> Result<()> {
    let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
    let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // SAFETY: valid handle (Drop waits for `worker_cmd` before destroying it); a NULL-terminated
    // array of NUL-terminated strings that outlive the call.
    let rc = unsafe { ffi::mpv_command(mpv.0, ptrs.as_mut_ptr()) };
    if rc < 0 {
        Err(Error::Player(format!("{}: {}", args[0], mpv_err(rc))))
    } else {
        Ok(())
    }
}

/// The height to ask for: the viewer's choice, or for Auto the smallest step of the ladder that
/// covers the size the video is drawn at.
fn preview_height(drawn: u32) -> u32 {
    match drawn {
        0..=144 => 144,
        145..=240 => 240,
        _ => 360,
    }
}

fn wanted_height(shared: &Shared) -> u32 {
    if shared.preview {
        let drawn = shared.target.lock().unwrap_or_else(|e| e.into_inner()).1;
        return preview_height(drawn);
    }
    let cap = crate::resolve::default_max_height();
    if let Some(h) = *shared.quality.lock().unwrap_or_else(|e| e.into_inner()) {
        return h;
    }
    let drawn = shared.target.lock().unwrap_or_else(|e| e.into_inner()).1;
    crate::resolve::HEIGHTS
        .iter()
        .copied()
        .find(|h| *h >= drawn)
        .unwrap_or(cap)
        .min(cap)
}

/// Find the streams on a core worker and hand them to mpv, starting at `start` seconds if given.
/// The answer is dropped if another load or a stop happened meanwhile (`ticket`).
fn begin_load(
    mpv: &Arc<Handle>,
    shared: &Arc<Shared>,
    video_id: &str,
    start: Option<f64>,
    ticket: u64,
    use_prepared: bool,
) {
    let (mpv, shared, id) = (mpv.clone(), shared.clone(), video_id.to_string());
    let height = wanted_height(&shared);
    let audio = shared
        .audio_choice
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let candidate = if use_prepared {
        let mut slot = shared
            .prepared_next
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slot.take().and_then(|entry| {
            if entry.id == id && entry.height == height {
                Some(entry.task)
            } else {
                entry.task.abort();
                None
            }
        })
    } else {
        None
    };
    crate::client::runtime().spawn(async move {
        let prepared = if let Some(task) = candidate
            && let Ok(Ok((streams, at))) = task.await
            && at.elapsed() < std::time::Duration::from_secs(300)
        {
            Some(streams)
        } else {
            None
        };
        if shared.shutdown.load(Ordering::Acquire)
            || shared.load_ticket.load(Ordering::Acquire) != ticket
        {
            return;
        }
        let found = if let Some(streams) = prepared {
            Ok(streams)
        } else {
            crate::resolve::resolve_with_audio(&id, height, audio.as_deref()).await
        };
        let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
        if shared.shutdown.load(Ordering::Acquire)
            || shared.load_ticket.load(Ordering::Acquire) != ticket
        {
            return; // the player is gone, or another video was asked for meanwhile
        }
        let result = found.and_then(|s| {
            if shared.preview && (s.is_live || s.height > 360) {
                return Err(Error::Player("No preview stream at 360p or lower".into()));
            }
            {
                let mut st = shared.lock_state();
                st.is_live = s.is_live;
                st.height = s.height;
                st.heights = s.heights;
                st.captions = s.captions;
                st.caption_languages = s.caption_languages;
                st.title = s.title;
                st.channel = s.author;
                st.storyboard = s.storyboard;
                st.audio_tracks = s.audio_tracks;
                st.audio_track = s.audio_track;
                st.normalization_gain_db = s.normalization_gain_db;
            }
            open_input(
                &mpv,
                &shared,
                PlaybackInput {
                    video: s.video,
                    audio: if shared.preview { None } else { s.audio },
                },
                start,
            )
        });
        if let Err(e) = result {
            let message = match e {
                Error::Player(m) => m,
                other => other.to_string(),
            };
            shared.lock_state().status = Status::Failed(message);
        }
        shared.wake();
    });
}

fn event_loop(mpv: Arc<Handle>, shared: Arc<Shared>) {
    let mut started_entry = None;
    loop {
        // SAFETY: valid handle until `Player::drop` destroys it after joining this thread.
        let ev = unsafe { &*ffi::mpv_wait_event(mpv.0, -1.0) };
        let mut wake = false;
        match ev.event_id {
            ffi::EVENT_SHUTDOWN => return,
            ffi::EVENT_COMMAND_REPLY if ev.reply_userdata & AUDIO_REPLY != 0 => {
                finish_audio_switch(&mpv, &shared, ev.reply_userdata & !AUDIO_REPLY, ev.error);
                wake = true;
            }
            ffi::EVENT_START_FILE if !ev.data.is_null() => {
                // SAFETY: START_FILE carries an mpv_event_start_file on libmpv 2.
                let start = unsafe { &*ev.data.cast::<ffi::mpv_event_start_file>() };
                started_entry = Some(start.playlist_entry_id);
                let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
                if started_entry == current_playlist_entry(&mpv) {
                    shared.clear_frame();
                    shared.lock_state().status = Status::Loading;
                    wake = true;
                }
            }
            ffi::EVENT_FILE_LOADED | ffi::EVENT_PLAYBACK_RESTART | ffi::EVENT_VIDEO_RECONFIG => {
                let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
                let reconfig = ev.event_id == ffi::EVENT_VIDEO_RECONFIG;
                // Before the first restart a reconfiguration can still describe the old VO.
                if reconfig && !shared.restarted.load(Ordering::Acquire) {
                    continue;
                }
                // A stop or replacement can overtake events queued for the previous file.
                if started_entry.is_none()
                    || started_entry != current_playlist_entry(&mpv)
                    || shared.shutdown.load(Ordering::Acquire)
                {
                    continue;
                }
                if ev.event_id == ffi::EVENT_FILE_LOADED {
                    let source = shared
                        .caption_source
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(input) = source.as_deref()
                        && command(&mpv, &["sub-add", input, "select"]).is_err()
                    {
                        shared.lock_state().selected_captions = None;
                    }
                    shared.accept_frames.store(true, Ordering::Release);
                } else {
                    // Refresh at restart even if neither dimension changed from the last file.
                    let size = video_display_size(&mpv);
                    let mut state = shared.lock_state();
                    wake = state.video_size != size;
                    state.video_size = size;
                    drop(state);
                    shared.restarted.store(true, Ordering::Release);
                    if wake {
                        shared.signal_render();
                    }
                }
                let mut s = shared.lock_state();
                if !reconfig && matches!(s.status, Status::Loading | Status::Ended) {
                    s.status = Status::Ready;
                    wake = true;
                }
            }
            ffi::EVENT_END_FILE if !ev.data.is_null() => {
                started_entry = None;
                // SAFETY: END_FILE carries an mpv_event_end_file.
                let end = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                let mut s = shared.lock_state();
                s.video_size = None;
                // A stream link that expired or a connection that dropped ends the file with an
                // error part-way through. That is not the end of the video: find fresh links and
                // carry on from the same place, up to three times per video.
                let resume = (end.reason == ffi::END_FILE_REASON_ERROR
                    && s.position > 1.0
                    && shared.recoveries.fetch_add(1, Ordering::AcqRel) < 3
                    && std::env::var("NT_VIDEO_OVERRIDE").map_or(true, |p| p.is_empty()))
                .then(|| (s.source.clone(), s.position));
                if let Some((Some(id), at)) = resume {
                    s.status = Status::Loading;
                    s.buffering = true;
                    drop(s);
                    let ticket = shared.load_ticket.fetch_add(1, Ordering::AcqRel) + 1;
                    begin_load(&mpv, &shared, &id, Some(at), ticket, false);
                    shared.wake();
                    continue;
                }
                match end.reason {
                    ffi::END_FILE_REASON_ERROR => {
                        s.status = Status::Failed(format!(
                            "Couldn't play this video ({})",
                            mpv_err(end.error)
                        ))
                    }
                    ffi::END_FILE_REASON_EOF => s.status = Status::Ended,
                    _ => {}
                }
                wake = true;
            }
            ffi::EVENT_PROPERTY_CHANGE if !ev.data.is_null() => {
                // SAFETY: PROPERTY_CHANGE carries an mpv_event_property.
                let prop = unsafe { &*(ev.data as *const ffi::mpv_event_property) };
                wake = apply_property(&shared, ev.reply_userdata as usize, prop);
                if wake
                    && OBSERVED
                        .get(ev.reply_userdata as usize)
                        .is_some_and(|obs| obs.0 == "time-pos")
                {
                    shared.remember_playback(false);
                }
            }
            _ => {}
        }
        if wake {
            shared.wake();
        }
    }
}

/// Fold one observed property into the state. Returns whether the UI should hear about it.
fn apply_property(shared: &Shared, index: usize, prop: &ffi::mpv_event_property) -> bool {
    let Some((name, _)) = OBSERVED.get(index) else {
        return false;
    };
    // An absent value (e.g. no duration yet) comes as FORMAT_NONE with null data.
    let (flag, double) = if prop.data.is_null() {
        (None, None)
    } else {
        // SAFETY: the data pointer's type follows `format`, which we requested per property.
        unsafe {
            match prop.format {
                ffi::FORMAT_FLAG => (Some(*(prop.data as *const c_int) != 0), None),
                ffi::FORMAT_DOUBLE => (None, Some(*(prop.data as *const f64))),
                _ => (None, None),
            }
        }
    };
    let mut s = shared.lock_state();
    let before = (
        s.position.floor(),
        s.duration,
        s.paused,
        s.buffering,
        s.volume,
        s.muted,
        s.status.clone(),
    );
    match *name {
        // Keep the last known position when mpv has none (between files, after an error): the
        // recovery path resumes from it.
        "time-pos" => s.position = double.unwrap_or(s.position),
        "duration" => s.duration = double.unwrap_or(0.0),
        "pause" => s.paused = flag.unwrap_or(false),
        "paused-for-cache" => s.buffering = flag.unwrap_or(false),
        "volume" => s.volume = double.unwrap_or(s.volume),
        "mute" => s.muted = flag.unwrap_or(false),
        "demuxer-cache-duration" => s.buffered_ahead = double.unwrap_or(0.0),
        "speed" => s.speed = double.unwrap_or(1.0),
        "eof-reached" => {
            if flag == Some(true) && s.status == Status::Ready {
                s.status = Status::Ended;
            } else if flag == Some(false) && s.status == Status::Ended {
                s.status = Status::Ready;
            }
        }
        _ => {}
    }
    let after = (
        s.position.floor(),
        s.duration,
        s.paused,
        s.buffering,
        s.volume,
        s.muted,
        s.status.clone(),
    );
    // Position changes wake the UI once a second (the clock), not on every picture.
    before != after
}

/// Fit the video's display size into the target box without upscaling. Width is a multiple of 16
/// so rows stay 64-byte aligned and tightly packed.
fn render_size(video: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let (vw, vh) = (video.0.max(1) as f64, video.1.max(1) as f64);
    let scale = (target.0 as f64 / vw).min(target.1 as f64 / vh).min(1.0);
    let w = ((vw * scale) as u32 / 16 * 16).max(16);
    let h = ((w as f64 * vh / vw).round() as u32).max(2) & !1;
    (w, h)
}

fn software_context(mpv: &Handle, shared: &Arc<Shared>) -> Result<RenderCtx> {
    let api = cstr("sw");
    let mut params = [
        ffi::mpv_render_param {
            kind: ffi::RENDER_PARAM_API_TYPE,
            data: api.as_ptr().cast_mut().cast(),
        },
        ffi::mpv_render_param {
            kind: ffi::RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    create_context(mpv, shared, &mut params)
}

fn create_context(
    mpv: &Handle,
    shared: &Arc<Shared>,
    params: &mut [ffi::mpv_render_param],
) -> Result<RenderCtx> {
    let mut ctx = std::ptr::null_mut();
    // SAFETY: mpv is live, and the private callers provide a terminated parameter list.
    let rc = unsafe { ffi::mpv_render_context_create(&mut ctx, mpv.0, params.as_mut_ptr()) };
    if rc < 0 || ctx.is_null() {
        return Err(Error::Player(format!(
            "mpv_render_context_create: {}",
            mpv_err(rc)
        )));
    }
    // SAFETY: the context is freed before the last Arc<Shared> is dropped.
    unsafe {
        ffi::mpv_render_context_set_update_callback(
            ctx,
            Some(on_render_update),
            Arc::as_ptr(shared).cast_mut().cast(),
        )
    };
    Ok(RenderCtx(ctx))
}

fn free_context(ctx: &RenderCtx) {
    // SAFETY: this worker exclusively owns the context with the GL context current, if required.
    unsafe {
        ffi::mpv_render_context_set_update_callback(ctx.0, None, std::ptr::null_mut());
        ffi::mpv_render_context_free(ctx.0);
    }
}

#[cfg(target_os = "linux")]
fn reconfigure_video(mpv: &Handle, shared: &Shared, gpu: bool) {
    let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
    if shared.shutdown.load(Ordering::Acquire) {
        return;
    }
    let has_surface = shared
        .surface
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    if has_surface != gpu {
        return;
    }
    let _ = command(
        mpv,
        &[
            "set",
            "hwdec",
            if gpu { "auto" } else { "vaapi-copy,auto-copy" },
        ],
    );
    // libmpv disables video when its active context is freed. Reopen the same signed
    // input at its current position. No new playback-metadata request is needed.
    let position = {
        let mut st = shared.lock_state();
        if !matches!(st.status, Status::Ready | Status::Ended) {
            return;
        }
        st.status = Status::Loading;
        st.position
    };
    let input = shared
        .input
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(input) = input
        && let Err(e) = open_input(mpv, shared, input, Some(position))
    {
        shared.lock_state().status = Status::Failed(e.to_string());
    }
}

fn render_loop(ctx: RenderCtx, _mpv: Arc<Handle>, shared: Arc<Shared>, format: PixelFormat) {
    #[cfg(target_os = "linux")]
    let mut gpu = None;
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut software = Some(ctx);
    let fmt = cstr(match format {
        PixelFormat::Rgba => "rgb0",
        PixelFormat::Bgra => "bgr0",
    });
    // Up to four buffers: the one on screen, the one being uploaded, the one being drawn, a spare.
    let mut pool: Vec<Arc<VideoFrame>> = Vec::new();
    loop {
        {
            let (lock, cv) = &shared.render_signal;
            let mut pending = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !*pending && !shared.shutdown.load(Ordering::Acquire) {
                pending = cv.wait(pending).unwrap_or_else(|e| e.into_inner());
            }
            *pending = false;
        }
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        #[cfg(target_os = "linux")]
        {
            video_surface::update_renderer(&_mpv, &shared, &mut software, &mut gpu);
            if let Some(renderer) = gpu.as_mut() {
                match renderer.render(&shared) {
                    Ok(()) => {
                        pool.clear();
                        continue;
                    }
                    Err(e) => video_surface::use_software(
                        &_mpv,
                        &shared,
                        &mut software,
                        &mut gpu,
                        Some(e.to_string()),
                    ),
                }
            }
        }
        let Some(ctx) = &software else {
            continue;
        };
        // Leave the pending update for the first frame until its size is available.
        let Some(video) = shared.lock_state().video_size else {
            continue;
        };
        // SAFETY: the context is only used on this thread.
        let flags = unsafe { ffi::mpv_render_context_update(ctx.0) };
        if flags & ffi::RENDER_UPDATE_FRAME == 0 {
            continue;
        }
        let target = *shared.target.lock().unwrap_or_else(|e| e.into_inner());
        let (w, h) = render_size(video, target);
        let len = (w * h * 4) as usize;

        // Reuse a pooled buffer nobody else holds; otherwise allocate.
        let reusable = pool
            .iter()
            .position(|f| Arc::strong_count(f) == 1 && f.width == w && f.height == h);
        let mut frame = match reusable {
            Some(i) => pool.swap_remove(i),
            None => Arc::new(VideoFrame {
                width: w,
                height: h,
                format,
                generation: 0,
                buf: AlignedBuf::new(len),
            }),
        };
        pool.retain(|f| Arc::strong_count(f) > 1 || (f.width == w && f.height == h));
        let Some(f) = Arc::get_mut(&mut frame) else {
            continue;
        };

        let started = Instant::now();
        let mut size = [w as c_int, h as c_int];
        let mut stride: usize = (w * 4) as usize;
        let mut params = [
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_SW_FORMAT,
                data: fmt.as_ptr() as *mut c_void,
            },
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_SW_STRIDE,
                data: (&raw mut stride).cast(),
            },
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_SW_POINTER,
                data: f.buf.as_mut_slice().as_mut_ptr().cast(),
            },
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        // SAFETY: the buffer holds at least stride * h bytes and is 64-byte aligned; params end with INVALID.
        let rc = unsafe { ffi::mpv_render_context_render(ctx.0, params.as_mut_ptr()) };
        if rc < 0 {
            pool.push(frame);
            continue;
        }
        // "rgb0"/"bgr0" leave the fourth byte undefined; make it opaque.
        for px in f.buf.as_mut_slice()[..len].as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        f.generation = shared.generation.load(Ordering::Acquire) + 1;
        {
            let mut st = shared.stats.lock().unwrap_or_else(|e| e.into_inner());
            st.frames += 1;
            st.total_render_ms += ms;
            st.max_render_ms = st.max_render_ms.max(ms);
        }
        // The previous frame stays in the pool (it was pushed when it was rendered).
        // `load`/`stop` clear the slot and close the gate under this same lock, so a picture of
        // the previous video that finished rendering late can never land after they ran.
        {
            let mut slot = shared.frame.lock().unwrap_or_else(|e| e.into_inner());
            let state = shared.lock_state();
            if !shared.accept_frames.load(Ordering::Acquire) || state.video_size != Some(video) {
                drop(state);
                drop(slot);
                pool.push(frame);
                continue;
            }
            slot.replace(frame.clone());
            shared.generation.fetch_add(1, Ordering::AcqRel);
        }
        pool.push(frame);
        while pool.len() > 4 {
            match pool.iter().position(|f| Arc::strong_count(f) == 1) {
                Some(i) => drop(pool.swap_remove(i)),
                None => break,
            }
        }
        crate::metrics::video_frame();
        shared.wake();
    }
    #[cfg(target_os = "linux")]
    video_surface::close_renderer(&shared, gpu);
    if let Some(ctx) = software {
        free_context(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::{preview_height, render_size, supports_modern_loadfile};

    #[test]
    fn previews_use_a_low_resolution_ladder() {
        assert_eq!(preview_height(100), 144);
        assert_eq!(preview_height(180), 240);
        assert_eq!(preview_height(720), 360);
    }

    #[test]
    fn modern_mpv_version() {
        assert!(!supports_modern_loadfile("mpv 0.37.6"));
        assert!(supports_modern_loadfile("mpv 0.38.0"));
        assert!(supports_modern_loadfile("mpv v0.42.0"));
        assert!(!supports_modern_loadfile("mpv v0.37.1"));
        assert!(supports_modern_loadfile("0.42.0-12-gabc"));
        assert!(supports_modern_loadfile("mpv 1.0"));
    }

    #[test]
    fn sizes() {
        assert_eq!(render_size((1920, 1080), (1280, 720)), (1280, 720));
        assert_eq!(render_size((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(render_size((1280, 720), (640, 640)), (640, 360));
        let (w, h) = render_size((1080, 1920), (1280, 720));
        assert_eq!(w % 16, 0);
        assert!(h <= 720 && h % 2 == 0);
    }
}
