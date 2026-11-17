//! Video playback through libmpv, shared by every frontend.
//!
//! [`crate::resolve`] finds the stream URLs in Rust, the way YouTube's native apps
//! do. libmpv (with all scripting off: no Lua, no yt-dlp) fetches and decodes them
//! (using the GPU decoder where it can, copied back), plays the sound itself, and renders
//! each picture with its software renderer into a CPU buffer on a thread of its
//! own. Frontends read the newest frame with [`Player::frame`] and upload it
//! however their toolkit draws images. Every frontend gets the same frames from
//! the same pipeline, so a comparison measures the UI toolkit, not the decoder.

use crate::error::{Error, Result};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

#[allow(non_camel_case_types)]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    pub enum mpv_handle {}
    pub enum mpv_render_context {}

    pub const FORMAT_FLAG: c_int = 3;
    pub const FORMAT_INT64: c_int = 4;
    pub const FORMAT_DOUBLE: c_int = 5;

    pub const EVENT_SHUTDOWN: c_int = 1;
    pub const EVENT_START_FILE: c_int = 6;
    pub const EVENT_END_FILE: c_int = 7;
    pub const EVENT_FILE_LOADED: c_int = 8;
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
        let names: &[&str] = if cfg!(target_os = "macos") {
            &[
                "libmpv.2.dylib",
                "libmpv.dylib",
                "/opt/homebrew/lib/libmpv.dylib",
                "/usr/local/lib/libmpv.dylib",
            ]
        } else {
            &["libmpv.so.2", "libmpv.so"]
        };
        for name in names {
            let c = std::ffi::CString::new(*name).map_err(|e| e.to_string())?;
            // SAFETY: a NUL-terminated name; the handle is never closed (libmpv stays loaded).
            let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if !h.is_null() {
                return Ok(h);
            }
        }
        Err("libmpv is not installed (looked for libmpv.so.2 / libmpv.dylib)".to_string())
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
        fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
        fn mpv_observe_property(ctx: *mut mpv_handle, id: u64, name: *const c_char, format: c_int) -> c_int;
        fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
        fn mpv_error_string(error: c_int) -> *const c_char;
        fn mpv_render_context_create(res: *mut *mut mpv_render_context, mpv: *mut mpv_handle, params: *mut mpv_render_param) -> c_int;
        fn mpv_render_context_set_update_callback(ctx: *mut mpv_render_context, callback: Option<unsafe extern "C" fn(*mut c_void)>, callback_ctx: *mut c_void);
        fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;
        fn mpv_render_context_render(ctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
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
    /// The video's display size, once known.
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
    /// Title and channel of what is playing, once the stream is found (empty for local files).
    pub title: String,
    pub channel: String,
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

struct Shared {
    state: Mutex<PlaybackState>,
    frame: Mutex<Option<Arc<VideoFrame>>>,
    generation: AtomicU64,
    waker: Mutex<Option<Waker>>,
    /// Signalled by mpv's render update callback, a resize, or shutdown.
    render_signal: (Mutex<bool>, Condvar),
    shutdown: AtomicBool,
    /// False between a `load`/`stop` and the new file being loaded, so a picture of the
    /// previous video that mpv renders late is never published as the new one's.
    accept_frames: AtomicBool,
    /// The viewer's quality choice: `None` is Auto (from the drawn size), `Some(h)` a fixed height.
    quality: Mutex<Option<u32>>,
    /// Automatic recoveries used for the current video (expired link, dropped connection).
    recoveries: AtomicU64,
    /// Bumped by every `load` and `stop`; a resolve that finishes for an older ticket is dropped.
    load_ticket: AtomicU64,
    /// Held while issuing a command from a worker, so `Drop` can't free mpv underneath it.
    worker_cmd: Mutex<()>,
    /// Largest picture to render, in physical pixels. Default 1280x720.
    target: Mutex<(u32, u32)>,
    stats: Mutex<RenderStats>,
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
        *slot = None;
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
    ("dwidth", ffi::FORMAT_INT64),
    ("dheight", ffi::FORMAT_INT64),
    ("demuxer-cache-duration", ffi::FORMAT_DOUBLE),
    ("eof-reached", ffi::FORMAT_FLAG),
    ("speed", ffi::FORMAT_DOUBLE),
];

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
        ffi::load().map_err(Error::Player)?;
        // SAFETY: plain constructor; null on failure.
        let raw = unsafe { ffi::mpv_create() };
        if raw.is_null() {
            return Err(Error::Player("mpv_create failed".into()));
        }
        let mpv = Arc::new(Handle(raw));
        let volume = crate::library::library().volume();
        let options = [
            ("vo", "libmpv".to_string()),
            // VA-API first: on hybrid laptops it decodes on the GPU that drives the display; "auto"
            // picked NVDEC here, which wakes the discrete GPU and took ~950 ms to start instead
            // of ~210 ms. Falls back to whatever else works.
            ("hwdec", "vaapi-copy,auto-copy".to_string()),
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
            .chain(extra);
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

        let shared = Arc::new(Shared {
            state: Mutex::new(PlaybackState {
                speed: 1.0,
                volume,
                ..Default::default()
            }),
            frame: Mutex::new(None),
            generation: AtomicU64::new(0),
            waker: Mutex::new(Some(Arc::new(waker))),
            render_signal: (Mutex::new(false), Condvar::new()),
            shutdown: AtomicBool::new(false),
            accept_frames: AtomicBool::new(false),
            quality: Mutex::new(None),
            recoveries: AtomicU64::new(0),
            load_ticket: AtomicU64::new(0),
            worker_cmd: Mutex::new(()),
            target: Mutex::new((1280, 720)),
            stats: Mutex::new(RenderStats::default()),
            changed: tokio::sync::Notify::new(),
        });

        let api = cstr("sw");
        let mut params = [
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_API_TYPE,
                data: api.as_ptr() as *mut c_void,
            },
            ffi::mpv_render_param {
                kind: ffi::RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut ctx: *mut ffi::mpv_render_context = std::ptr::null_mut();
        // SAFETY: valid handle; params end with INVALID.
        let rc = unsafe { ffi::mpv_render_context_create(&mut ctx, raw, params.as_mut_ptr()) };
        if rc < 0 || ctx.is_null() {
            // SAFETY: nothing else holds the handle yet.
            unsafe { ffi::mpv_terminate_destroy(raw) };
            return Err(Error::Player(format!(
                "mpv_render_context_create: {}",
                mpv_err(rc)
            )));
        }
        // SAFETY: `shared` outlives the render context: the render thread frees the context before it exits,
        // and the thread holds its own `Arc<Shared>` until then.
        unsafe {
            ffi::mpv_render_context_set_update_callback(
                ctx,
                Some(on_render_update),
                Arc::as_ptr(&shared) as *mut c_void,
            )
        };

        let render_thread = {
            let shared = shared.clone();
            let ctx = RenderCtx(ctx);
            std::thread::Builder::new()
                .name("nt-player-render".into())
                .spawn(move || render_loop(ctx, shared, format))
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
        crate::mpris::start(Remote {
            mpv: mpv.clone(),
            shared: shared.clone(),
        });
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

    /// Open a YouTube video by id and start playing. Returns at once: the streams are found on a
    /// core worker ([`crate::resolve`]) while the state says `Loading`; a failure becomes
    /// `Status::Failed`. `NT_VIDEO_OVERRIDE=<file>` plays that file instead (benchmarks use it to
    /// take the network out of the measurement).
    pub fn load(&self, video_id: &str) -> Result<()> {
        let ticket = self.shared.load_ticket.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.clear_frame();
        {
            let mut s = self.shared.lock_state();
            let (volume, muted) = (s.volume, s.muted);
            *s = PlaybackState {
                status: Status::Loading,
                volume,
                muted,
                source: Some(video_id.to_string()),
                ..Default::default()
            };
        }
        self.set_flag("pause", false);
        // Unload whatever played before, so its last picture and sound stop while we resolve.
        let _ = self.command(&["stop"]);
        self.shared.wake();
        if let Ok(path) = std::env::var("NT_VIDEO_OVERRIDE")
            && !path.is_empty()
        {
            return self.command(&["loadfile", &path, "replace"]);
        }
        self.shared.recoveries.store(0, Ordering::Release);
        begin_load(&self.mpv, &self.shared, video_id, None, ticket);
        Ok(())
    }

    /// Stop and unload; frees the decoder and the stream.
    pub fn stop(&self) {
        self.shared.load_ticket.fetch_add(1, Ordering::AcqRel);
        self.shared.clear_frame();
        let _ = self.command(&["stop"]);
        {
            // Volume and mute are mpv's settings, not the file's: they survive a stop.
            let mut st = self.shared.lock_state();
            let (volume, muted) = (st.volume, st.muted);
            *st = PlaybackState {
                volume,
                muted,
                ..Default::default()
            };
        }
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
            begin_load(&self.mpv, &self.shared, &id, Some(st.position), ticket);
        }
    }

    pub fn quality(&self) -> Option<u32> {
        *self
            .shared
            .quality
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 0-100. Remembered across launches.
    pub fn set_volume(&self, volume: f64) {
        let v = volume.clamp(0.0, 100.0);
        self.set_double("volume", v);
        crate::library::library().set_volume(v);
    }

    /// With `NT_MUTE` set (every test and benchmark run) the player cannot be unmuted.
    pub fn set_muted(&self, muted: bool) {
        self.set_flag("mute", muted || force_muted());
    }

    /// The largest picture worth rendering, in physical pixels (the size the video is drawn at).
    /// Rendering is capped at the video's own size, so a big box doesn't upscale on the CPU.
    pub fn set_target_size(&self, width: u32, height: u32) {
        let new = (width.clamp(64, 3840), height.clamp(36, 2160));
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

    pub fn render_stats(&self) -> RenderStats {
        *self.shared.stats.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
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
fn wanted_height(shared: &Shared) -> u32 {
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
) {
    let (mpv, shared, id) = (mpv.clone(), shared.clone(), video_id.to_string());
    let height = wanted_height(&shared);
    crate::client::runtime().spawn(async move {
        let found = crate::resolve::resolve(&id, height).await;
        let _guard = shared.worker_cmd.lock().unwrap_or_else(|e| e.into_inner());
        if shared.shutdown.load(Ordering::Acquire)
            || shared.load_ticket.load(Ordering::Acquire) != ticket
        {
            return; // the player is gone, or another video was asked for meanwhile
        }
        let result = found.and_then(|s| {
            {
                let mut st = shared.lock_state();
                st.is_live = s.is_live;
                st.height = s.height;
                st.heights = s.heights.clone();
                st.title = s.title.clone();
                st.channel = s.author.clone();
            }
            // %N% quotes a value of N bytes, so commas and '=' in a URL are safe.
            let mut options = Vec::new();
            if let Some(audio) = &s.audio {
                options.push(format!("audio-file=%{}%{}", audio.len(), audio));
            }
            if let Some(t) = start.filter(|t| *t > 0.5 && !s.is_live) {
                options.push(format!("start={t:.3}"));
            }
            let options = options.join(",");
            let mut args = vec!["loadfile", s.video.as_str(), "replace", "-1"];
            if !options.is_empty() {
                args.push(&options);
            }
            command(&mpv, &args)
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
    loop {
        // SAFETY: valid handle until `Player::drop` destroys it after joining this thread.
        let ev = unsafe { &*ffi::mpv_wait_event(mpv.0, -1.0) };
        let mut wake = false;
        match ev.event_id {
            ffi::EVENT_SHUTDOWN => return,
            ffi::EVENT_START_FILE => {
                shared.clear_frame();
                shared.lock_state().status = Status::Loading;
                wake = true;
            }
            ffi::EVENT_FILE_LOADED | ffi::EVENT_PLAYBACK_RESTART => {
                if ev.event_id == ffi::EVENT_FILE_LOADED {
                    shared.accept_frames.store(true, Ordering::Release);
                }
                let mut s = shared.lock_state();
                if matches!(s.status, Status::Loading | Status::Ended) {
                    s.status = Status::Ready;
                    wake = true;
                }
            }
            ffi::EVENT_END_FILE if !ev.data.is_null() => {
                // SAFETY: END_FILE carries an mpv_event_end_file.
                let end = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                let mut s = shared.lock_state();
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
                    begin_load(&mpv, &shared, &id, Some(at), ticket);
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
    let (flag, int, double) = if prop.data.is_null() {
        (None, None, None)
    } else {
        // SAFETY: the data pointer's type follows `format`, which we requested per property.
        unsafe {
            match prop.format {
                ffi::FORMAT_FLAG => (Some(*(prop.data as *const c_int) != 0), None, None),
                ffi::FORMAT_INT64 => (None, Some(*(prop.data as *const i64)), None),
                ffi::FORMAT_DOUBLE => (None, None, Some(*(prop.data as *const f64))),
                _ => (None, None, None),
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
        s.video_size,
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
        "dwidth" | "dheight" => {
            let (mut w, mut h) = s.video_size.unwrap_or((0, 0));
            let v = int.unwrap_or(0).max(0) as u32;
            if *name == "dwidth" {
                w = v
            } else {
                h = v
            }
            // Keep the half we have until the other arrives; readers check both sides are non-zero.
            s.video_size = (w > 0 || h > 0).then_some((w, h));
        }
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
        s.video_size,
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

fn render_loop(ctx: RenderCtx, shared: Arc<Shared>, format: PixelFormat) {
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
        // SAFETY: the context is only used on this thread.
        let flags = unsafe { ffi::mpv_render_context_update(ctx.0) };
        if flags & ffi::RENDER_UPDATE_FRAME == 0 {
            continue;
        }
        let video = shared
            .lock_state()
            .video_size
            .filter(|(w, h)| *w > 0 && *h > 0)
            .unwrap_or((1280, 720));
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
            if !shared.accept_frames.load(Ordering::Acquire) {
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
    // SAFETY: last use of the context, on the thread that used it; `shared` (the callback's data) is still alive.
    unsafe { ffi::mpv_render_context_free(ctx.0) };
}

#[cfg(test)]
mod tests {
    use super::render_size;

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
