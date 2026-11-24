//! Wayland video surface for the libmpv OpenGL renderer.

use super::{Handle, RenderCtx, Shared, cstr, ffi, mpv_err};
use crate::{Error, Player, Result};
use khronos_egl as egl;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::{Arc, Condvar, Mutex};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
    wl_subcompositor::WlSubcompositor, wl_subsurface::WlSubsurface, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

/// A lease on a Wayland child surface below the application's window.
pub struct VideoSurface {
    player: Arc<Shared>,
    control: Arc<Control>,
}

pub(super) struct Slot {
    pub control: Arc<Control>,
    pub target: Option<Target>,
}

pub(super) struct Control {
    view: Mutex<View>,
    status: Mutex<SurfaceStatus>,
    status_changed: Condvar,
    subsurface: WlSubsurface,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    /// The part of the rectangle that is shown, in logical pixels relative to its corner
    /// (x, y, width, height); `None` shows all of it.
    clip: Option<(u32, u32, u32, u32)>,
    scale: f64,
    visible: bool,
    detach: bool,
}

impl View {
    /// The shown part, clamped to the rectangle.
    fn shown(&self) -> (u32, u32, u32, u32) {
        match self.clip {
            None => (0, 0, self.width, self.height),
            Some((x, y, w, h)) => {
                let x = x.min(self.width);
                let y = y.min(self.height);
                (x, y, w.min(self.width - x), h.min(self.height - y))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SurfaceStatus {
    Starting,
    Ready { presenting: bool },
    Detached(Option<String>),
}

impl VideoSurface {
    /// Attach to a Wayland window. The render worker initializes EGL and libmpv OpenGL.
    ///
    /// # Safety
    /// Both handles must be live and refer to the same Wayland connection and parent.
    /// Detach the surface before the toolkit destroys either handle.
    pub unsafe fn attach(
        player: &Player,
        display: RawDisplayHandle,
        window: RawWindowHandle,
        scale: f64,
    ) -> Result<Self> {
        if player.shared.preview {
            return Err(surface_error("preview players use software frames"));
        }
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) =
            (display, window)
        else {
            return Err(surface_error("the window is not from Wayland"));
        };
        let (control, shared) = {
            let mut slot = player
                .shared
                .surface
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if slot.is_some() {
                return Err(surface_error("this player already has a surface"));
            }
            // SAFETY: caller supplies a compatible parent and keeps the foreign handles alive.
            let target = unsafe { Target::new(display.display.as_ptr(), window.surface.as_ptr())? };
            let control = Arc::new(Control {
                view: Mutex::new(View {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                    clip: None,
                    scale: valid_scale(scale),
                    visible: true,
                    detach: false,
                }),
                status: Mutex::new(SurfaceStatus::Starting),
                status_changed: Condvar::new(),
                subsurface: target.subsurface.clone(),
            });
            *slot = Some(Slot {
                control: control.clone(),
                target: Some(target),
            });
            (control, player.shared.clone())
        };
        let surface = Self {
            player: shared,
            control,
        };
        // The render worker loads EGL and the GL driver (about 80 ms the first time, mostly the
        // driver's own libraries) while the caller goes on; software frames continue until the
        // first picture reaches the surface. Failures show up in `error()`.
        surface.player.signal_render();
        Ok(surface)
    }

    /// Set the destination in logical pixels, relative to the parent's upper-left corner.
    /// Repaint the parent after moving the rectangle to commit its new position.
    pub fn set_rect(&self, x: i32, y: i32, width: u32, height: u32) {
        self.update_view(|v| {
            v.x = x;
            v.y = y;
            v.width = width.min(8192);
            v.height = height.min(8192);
        });
    }

    /// Show only part of the rectangle, in logical pixels relative to its upper-left corner,
    /// for a player partly scrolled out of the window: the picture keeps its size and
    /// position and the child surface covers just the visible part (wp_viewport source and
    /// destination). `None` shows the whole rectangle. Repaint the parent afterwards.
    pub fn set_clip(&self, clip: Option<(u32, u32, u32, u32)>) {
        self.update_view(|v| v.clip = clip);
    }

    pub fn set_scale(&self, scale: f64) {
        self.update_view(|v| v.scale = valid_scale(scale));
    }

    pub fn set_visible(&self, visible: bool) {
        self.update_view(|v| v.visible = visible);
    }

    /// Whether the worker has initialized the GPU renderer.
    pub fn is_ready(&self) -> bool {
        matches!(
            *self
                .control
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            SurfaceStatus::Ready { .. }
        )
    }

    /// True once a video frame has reached the child. Paint a transparent hole only then.
    pub fn is_presenting(&self) -> bool {
        matches!(
            *self
                .control
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            SurfaceStatus::Ready { presenting: true }
        )
    }

    /// An EGL or Wayland failure returns playback to the software renderer.
    pub fn error(&self) -> Option<String> {
        let status = self
            .control
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match &*status {
            SurfaceStatus::Detached(message) => message.clone(),
            _ => None,
        }
    }

    /// Remove the child and release foreign handles before returning.
    pub fn detach(self) {
        drop(self);
    }

    fn update_view(&self, update: impl FnOnce(&mut View)) {
        let mut view = self.control.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.detach {
            return;
        }
        let previous = *view;
        update(&mut view);
        let changed = previous != *view;
        // The child's position is its shown part's corner (committed with the parent).
        let corner = |v: &View| {
            let (cx, cy, _, _) = v.shown();
            (v.x + cx as i32, v.y + cy as i32)
        };
        if corner(&previous) != corner(&view) {
            let (x, y) = corner(&view);
            self.control.subsurface.set_position(x, y);
        }
        drop(view);
        if changed {
            self.player.signal_render();
        }
    }
}

impl Drop for VideoSurface {
    fn drop(&mut self) {
        self.update_view(|v| v.detach = true);
        let status = self
            .control
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _status = self
            .control
            .status_changed
            .wait_while(status, |s| !matches!(s, SurfaceStatus::Detached(_)))
            .unwrap_or_else(|e| e.into_inner());
    }
}

impl Control {
    fn detaching(&self) -> bool {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).detach
    }

    fn started(&self) {
        self.set_status(SurfaceStatus::Ready { presenting: false });
    }

    fn finished(&self, error: Option<String>) {
        self.set_status(SurfaceStatus::Detached(error));
    }

    fn set_status(&self, value: SurfaceStatus) -> bool {
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *status != value;
        if changed {
            *status = value;
            self.status_changed.notify_all();
        }
        changed
    }
}

fn valid_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(0.5, 8.0)
    } else {
        1.0
    }
}

fn surface_error(context: impl std::fmt::Display) -> Error {
    Error::Player(format!("Wayland video surface: {context}"))
}

pub(super) struct Target {
    connection: Connection,
    queue: EventQueue<QueueState>,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
}

struct QueueState;
impl Dispatch<WlRegistry, GlobalListContents> for QueueState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(QueueState: ignore WlCompositor);
delegate_noop!(QueueState: ignore WlSubcompositor);
delegate_noop!(QueueState: ignore WlSurface);
delegate_noop!(QueueState: ignore WlSubsurface);
delegate_noop!(QueueState: ignore WlRegion);
delegate_noop!(QueueState: ignore WpViewporter);
delegate_noop!(QueueState: ignore WpViewport);

impl Target {
    unsafe fn new(display: *mut c_void, parent: *mut c_void) -> Result<Self> {
        // SAFETY: the caller owns the connection and keeps it live until detach.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);
        // SAFETY: the caller supplied a live wl_surface on that connection.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), parent.cast()) }
            .map_err(surface_error)?;
        let parent = WlSurface::from_id(&connection, id).map_err(surface_error)?;
        let (globals, queue) =
            registry_queue_init::<QueueState>(&connection).map_err(surface_error)?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).map_err(surface_error)?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).map_err(surface_error)?;
        let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).map_err(surface_error)?;
        let surface = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_desync();
        subsurface.place_below(&parent);
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        let input = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&input));
        input.destroy();
        surface.commit();
        viewporter.destroy();
        subcompositor.destroy();
        let result = Self {
            connection,
            queue,
            surface,
            subsurface,
            viewport,
        };
        result.connection.flush().map_err(surface_error)?;
        Ok(result)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let _ = self.connection.flush();
    }
}

type Egl = egl::DynamicInstance<egl::EGL1_5>;

pub(super) struct EglWindow {
    api: Box<Egl>,
    display: egl::Display,
    context: Option<egl::Context>,
    window_surface: Option<egl::Surface>,
    window: WlEglSurface,
}

impl EglWindow {
    fn create(target: &Target) -> Result<Self> {
        let window = WlEglSurface::new(target.surface.id(), 1, 1).map_err(surface_error)?;
        // SAFETY: EGL's dynamic loader validates the driver symbols for this interface.
        let api = Box::new(unsafe { Egl::load_required() }.map_err(surface_error)?);
        // EGL_PLATFORM_WAYLAND_KHR from EGL_KHR_platform_wayland.
        const WAYLAND: egl::Enum = 0x31D8;
        // SAFETY: this pointer is the live wl_display for the wl_egl_window above.
        let display = unsafe {
            api.get_platform_display(
                WAYLAND,
                target.connection.backend().display_ptr().cast(),
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(surface_error)?;
        let mut egl_window = Self {
            api,
            display,
            context: None,
            window_surface: None,
            window,
        };
        let api = &egl_window.api;
        api.initialize(display).map_err(surface_error)?;
        api.bind_api(egl::OPENGL_ES_API).map_err(surface_error)?;
        let config = api
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::WINDOW_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES2_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )
            .map_err(surface_error)?
            .ok_or_else(|| surface_error("no EGL window config"))?;
        let ctx = api
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
            )
            .map_err(surface_error)?;
        egl_window.context = Some(ctx);
        // SAFETY: window and display belong to the same Wayland platform and connection.
        let surface = unsafe {
            api.create_window_surface(display, config, egl_window.window.ptr().cast_mut(), None)
        }
        .map_err(surface_error)?;
        egl_window.window_surface = Some(surface);
        api.make_current(display, Some(surface), Some(surface), Some(ctx))
            .map_err(surface_error)?;
        api.swap_interval(display, 0).map_err(surface_error)?;
        Ok(egl_window)
    }

    fn create_renderer(
        self,
        mpv: &Handle,
        target: Target,
        shared: &Arc<Shared>,
        control: Arc<Control>,
    ) -> Result<GpuRenderer> {
        #[repr(C)]
        struct GlInit {
            get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
            user: *mut c_void,
        }
        unsafe extern "C" fn get_proc(user: *mut c_void, name: *const c_char) -> *mut c_void {
            // SAFETY: libmpv received a stable EGL instance and passes a NUL-terminated name.
            let (egl, name) = unsafe { (&*user.cast::<Egl>(), CStr::from_ptr(name)) };
            name.to_str()
                .ok()
                .and_then(|name| egl.get_proc_address(name))
                .map_or(std::ptr::null_mut(), |address| address as *mut c_void)
        }
        let api_name = cstr("opengl");
        let mut gl = GlInit {
            get_proc_address: get_proc,
            user: (&*self.api as *const Egl).cast_mut().cast(),
        };
        let mut advanced = 1 as c_int;
        let mut params = [
            ffi::MpvRenderParam {
                kind: ffi::RENDER_PARAM_API_TYPE,
                data: api_name.as_ptr().cast_mut().cast(),
            },
            ffi::MpvRenderParam {
                kind: 2,
                data: (&raw mut gl).cast(),
            }, // OPENGL_INIT_PARAMS
            ffi::MpvRenderParam {
                kind: 9,
                data: target.connection.backend().display_ptr().cast(),
            }, // WL_DISPLAY
            ffi::MpvRenderParam {
                kind: 10,
                data: (&raw mut advanced).cast(),
            }, // ADVANCED_CONTROL
            ffi::MpvRenderParam {
                kind: ffi::RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let ctx = match super::create_context(mpv, shared, &mut params) {
            Ok(ctx) => ctx,
            Err(e) => {
                drop(self);
                drop(target);
                return Err(e);
            }
        };
        Ok(GpuRenderer {
            ctx,
            egl: self,
            target,
            control,
            mapped: false,
            previous: None,
        })
    }
}

impl Drop for EglWindow {
    fn drop(&mut self) {
        let _ = self.api.make_current(self.display, None, None, None);
        if let Some(surface) = self.window_surface {
            let _ = self.api.destroy_surface(self.display, surface);
        }
        if let Some(context) = self.context {
            let _ = self.api.destroy_context(self.display, context);
        }
        // The toolkit may own this EGL display as well.
    }
}

pub(super) struct GpuRenderer {
    ctx: RenderCtx,
    egl: EglWindow,
    target: Target,
    pub control: Arc<Control>,
    mapped: bool,
    previous: Option<View>,
}

impl GpuRenderer {
    /// Whether the frame mpv will render next is a newly decoded picture, rather than a redraw
    /// of the last one or the black fill shown while no picture exists yet.
    fn next_is_new_picture(&self) -> bool {
        #[repr(C)]
        struct FrameInfo {
            flags: u64,
            target_time: i64,
        }
        const PRESENT: u64 = 1;
        const REDRAW: u64 = 1 << 1;
        let mut info = FrameInfo {
            flags: 0,
            target_time: 0,
        };
        let param = ffi::MpvRenderParam {
            kind: 11, // NEXT_FRAME_INFO
            data: (&raw mut info).cast(),
        };
        // SAFETY: this worker owns the context; mpv fills the struct NEXT_FRAME_INFO names.
        let rc = unsafe { ffi::mpv_render_context_get_info(self.ctx.0, param) };
        rc >= 0 && info.flags & PRESENT != 0 && info.flags & REDRAW == 0
    }

    pub(super) fn render(&mut self, shared: &Shared) -> Result<()> {
        let epoch = shared.file_epoch.load(std::sync::atomic::Ordering::Acquire);
        // SAFETY: EGL is current and this worker owns the render context.
        let flags = unsafe { ffi::mpv_render_context_update(self.ctx.0) };
        let view = *self.control.view.lock().unwrap_or_else(|e| e.into_inner());
        let (_, _, shown_w, shown_h) = view.shown();
        let show = view.visible
            && view.width > 0
            && view.height > 0
            && shown_w > 0
            && shown_h > 0
            && shared
                .accept_frames
                .load(std::sync::atomic::Ordering::Acquire);
        if !show {
            if self.mapped {
                self.target.surface.attach(None, 0, 0);
                self.target.surface.commit();
                self.mapped = false;
                self.target.connection.flush().map_err(surface_error)?;
                if self
                    .control
                    .set_status(SurfaceStatus::Ready { presenting: false })
                {
                    shared.wake();
                }
            }
            if flags & ffi::RENDER_UPDATE_FRAME != 0 {
                let mut skip = 1 as c_int;
                let mut params = [
                    ffi::MpvRenderParam {
                        kind: 13,
                        data: (&raw mut skip).cast(),
                    }, // SKIP_RENDERING
                    ffi::MpvRenderParam {
                        kind: ffi::RENDER_PARAM_INVALID,
                        data: std::ptr::null_mut(),
                    },
                ];
                // SAFETY: see above. Skip consumes the frame without swapping an EGL buffer.
                unsafe {
                    ffi::mpv_render_context_render(self.ctx.0, params.as_mut_ptr());
                }
            }
            return Ok(());
        }
        if self.mapped && self.previous == Some(view) && flags & ffi::RENDER_UPDATE_FRAME == 0 {
            return Ok(());
        }
        let pixels =
            |logical: u32| (f64::from(logical) * view.scale).round().clamp(1.0, 8192.0) as i32;
        let (width, height) = (pixels(view.width), pixels(view.height));
        if self.previous != Some(view) {
            // The buffer always holds the whole picture; the viewport shows the clipped part.
            // Source coordinates are buffer pixels and must stay inside the buffer.
            let (cx, cy, cw, ch) = view.shown();
            if (cx, cy, cw, ch) == (0, 0, view.width, view.height) {
                self.target.viewport.set_source(-1.0, -1.0, -1.0, -1.0);
            } else {
                let px = |logical: u32| (f64::from(logical) * view.scale).round() as i32;
                let sx = px(cx).clamp(0, width - 1);
                let sy = px(cy).clamp(0, height - 1);
                let sw = px(cw).clamp(1, width - sx);
                let sh = px(ch).clamp(1, height - sy);
                self.target.viewport.set_source(
                    f64::from(sx),
                    f64::from(sy),
                    f64::from(sw),
                    f64::from(sh),
                );
            }
            self.target
                .viewport
                .set_destination(cw.max(1) as i32, ch.max(1) as i32);
            self.egl.window.resize(width, height, 0, 0);
            self.previous = Some(view);
        }
        #[repr(C)]
        struct Fbo {
            name: c_int,
            width: c_int,
            height: c_int,
            format: c_int,
        }
        let mut fbo = Fbo {
            name: 0,
            width,
            height,
            format: 0,
        };
        let mut flip = 1 as c_int;
        let mut params = [
            ffi::MpvRenderParam {
                kind: 3,
                data: (&raw mut fbo).cast(),
            }, // OPENGL_FBO
            ffi::MpvRenderParam {
                kind: 4,
                data: (&raw mut flip).cast(),
            }, // FLIP_Y
            ffi::MpvRenderParam {
                kind: ffi::RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let new_picture = flags & ffi::RENDER_UPDATE_FRAME != 0 && self.next_is_new_picture();
        // SAFETY: the current default FBO belongs to this EGL window.
        let started = std::time::Instant::now();
        let result = unsafe { ffi::mpv_render_context_render(self.ctx.0, params.as_mut_ptr()) };
        if result < 0 {
            return Err(surface_error(mpv_err(result)));
        }
        let surface = self
            .egl
            .window_surface
            .ok_or_else(|| surface_error("missing EGL surface"))?;
        self.egl
            .api
            .swap_buffers(self.egl.display, surface)
            .map_err(surface_error)?;
        // SAFETY: notify mpv of the completed presentation on that same current context.
        unsafe { ffi::mpv_render_context_report_swap(self.ctx.0) };
        self.target
            .queue
            .dispatch_pending(&mut QueueState)
            .map_err(surface_error)?;
        self.target.connection.flush().map_err(surface_error)?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut stats = shared.stats.lock().unwrap_or_else(|e| e.into_inner());
            stats.frames += 1;
            stats.total_render_ms += ms;
            stats.max_render_ms = stats.max_render_ms.max(ms);
        }
        crate::metrics::video_frame();
        // A decoded picture of the current file (frames are accepted from its file-loaded on).
        // mpv shows the first one just before it reports playback-restart, so waiting for that
        // event here cost a whole frame interval (33 ms at 30 fps) on every switch.
        let counted = {
            use std::sync::atomic::Ordering;
            // Under the frame lock `clear_frame` takes, so a load or stop that happened while
            // this picture rendered (it belongs to the previous file) is never counted.
            let _slot = shared.frame.lock().unwrap_or_else(|e| e.into_inner());
            let current = shared.accept_frames.load(Ordering::Acquire)
                && shared.file_epoch.load(Ordering::Acquire) == epoch;
            let real = new_picture
                || (flags & ffi::RENDER_UPDATE_FRAME != 0
                    && shared.restarted.load(Ordering::Acquire));
            (current && real).then(|| shared.presented.fetch_add(1, Ordering::AcqRel))
        };
        if counted == Some(0) {
            // The first real picture of this file: the frontend may now cut its hole.
            shared.wake();
        }
        self.mapped = true;
        if self
            .control
            .set_status(SurfaceStatus::Ready { presenting: true })
        {
            *shared.frame.lock().unwrap_or_else(|e| e.into_inner()) = None;
            shared.wake();
        }
        Ok(())
    }
}

impl Drop for GpuRenderer {
    fn drop(&mut self) {
        // EGL is still live and current. The fields then drop EGL before Wayland.
        super::free_context(&self.ctx);
    }
}

// Inspired by Akshar Patel's yt-music-gpui native/video.c (MIT / Apache-2.0). The
// same worker owns both render modes so mpv has at most one context.
pub(super) fn update_renderer(
    mpv: &Arc<Handle>,
    shared: &Arc<Shared>,
    software: &mut Option<RenderCtx>,
    gpu: &mut Option<GpuRenderer>,
) {
    if let Some(renderer) = gpu.as_ref() {
        if renderer.control.detaching() {
            use_software(mpv, shared, software, gpu, None);
        }
        return;
    }
    let pending = {
        let mut slot = shared.surface.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_mut().and_then(|entry| {
            entry
                .target
                .take()
                .map(|target| (target, entry.control.clone()))
        })
    };
    let Some((target, control)) = pending else {
        return;
    };
    if control.detaching() {
        // Detached before the worker got to it.
        drop(target);
        finish(shared, None);
        return;
    }
    let trace = std::env::var_os("NT_GPU_TRACE")
        .is_some()
        .then(std::time::Instant::now);
    let egl = match EglWindow::create(&target) {
        Ok(egl) => egl,
        Err(e) => {
            drop(target);
            finish(shared, Some(e.to_string()));
            return;
        }
    };
    if let Some(ctx) = software.take() {
        super::free_context(&ctx);
    }
    match egl.create_renderer(mpv, target, shared, control.clone()) {
        Ok(renderer) => {
            *gpu = Some(renderer);
            if let Some(start) = trace {
                eprintln!(
                    "nt-core: GPU renderer initialized in {:.1} ms",
                    start.elapsed().as_secs_f64() * 1000.0
                );
            }
            control.started();
            // Ordinary libmpv calls stay off the render worker. The GL context can give the
            // decoder a GPU device, unlike the software renderer.
            if let Ok(rt) = crate::try_runtime() {
                let (mpv, shared) = (mpv.clone(), shared.clone());
                drop(rt.spawn_blocking(move || super::reconfigure_video(&mpv, &shared, true)));
            }
        }
        Err(e) => use_software(mpv, shared, software, gpu, Some(e.to_string())),
    }
}

pub(super) fn use_software(
    mpv: &Arc<Handle>,
    shared: &Arc<Shared>,
    software: &mut Option<RenderCtx>,
    gpu: &mut Option<GpuRenderer>,
    message: Option<String>,
) {
    drop(gpu.take());
    match super::software_context(mpv, shared) {
        Ok(ctx) => *software = Some(ctx),
        Err(e) => shared.lock_state().status = super::Status::Failed(e.to_string()),
    }
    finish(shared, message);
    match crate::client::try_runtime() {
        Ok(rt) => {
            let (mpv, shared) = (mpv.clone(), shared.clone());
            rt.spawn_blocking(move || super::reconfigure_video(&mpv, &shared, false));
        }
        // No runtime to hand it to: reconfigure here instead (slower, but never lost).
        Err(_) => super::reconfigure_video(mpv, shared, false),
    }
}

pub(super) fn close_renderer(shared: &Shared, gpu: Option<GpuRenderer>) {
    drop(gpu);
    finish(shared, None);
}

fn finish(shared: &Shared, error: Option<String>) {
    let removed = shared
        .surface
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(Slot { control, target }) = removed {
        drop(target);
        control.finished(error);
        // At shutdown, a UI might be waiting for this thread to join.
        if !shared.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            shared.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sanitize_fractional_scale() {
        assert_eq!(super::valid_scale(1.25), 1.25);
        assert_eq!(super::valid_scale(f64::NAN), 1.0);
        assert_eq!(super::valid_scale(1000.0), 8.0);
    }
}
