//! Window-owned Wayland video lease. Other backends keep the Pixels path.

use crate::app::{App, handle};
use crate::theme::{TOP_BAR, t};
use libloading::Library;
use nt_core::{Player, VideoSurface};
use rune::raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle};
use rune::{Color, El, Live, WindowHandle, el};
use std::ffi::c_void;
use std::ptr::NonNull;

pub struct NativeVideo {
    window: Option<WindowHandle>,
    tried: bool,
    lease: Option<VideoSurface>,
    pub showing: bool,
    masks: [Live<[f32; 4]>; 4],
}

impl Default for NativeVideo {
    fn default() -> Self {
        Self {
            window: None,
            tried: false,
            lease: None,
            showing: false,
            masks: std::array::from_fn(|_| Live::new([0.0; 4])),
        }
    }
}

pub fn window_created(window: &WindowHandle) {
    if let Some(app) = handle() {
        let window = window.clone();
        app.update_if(move |s, _| {
            s.native_video.window = Some(window);
            s.native_video.tried = false;
            false
        });
    }
}

pub fn enabled() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("NT_RUNE_SOFTWARE").is_none()
}

fn attach(window: &WindowHandle, player: &Player, scale: f32) -> Result<VideoSurface, String> {
    window.with_raw_handle(|handle| {
        let raw = handle.as_raw();
        let RawWindowHandle::Wayland(surface) = raw else {
            return Err("Rune did not create a Wayland window".into());
        };
        // Rune exposes the window handle but no display handle. Wayland 1.23 added
        // wl_proxy_get_display so the display can be recovered from this exact surface.
        // SAFETY: this loads the client library already backing winit's live wl_surface.
        let library =
            unsafe { Library::new("libwayland-client.so.0") }.map_err(|e| e.to_string())?;
        // SAFETY: libwayland declares this symbol as wl_display *(wl_proxy *).
        let get_display = unsafe {
            library.get::<unsafe extern "C" fn(*mut c_void) -> *mut c_void>(b"wl_proxy_get_display")
        }
        .map_err(|e| format!("Wayland 1.23 display accessor unavailable: {e}"))?;
        // SAFETY: the borrowed handle holds its parent alive for this synchronous attach.
        let display = NonNull::new(unsafe { get_display(surface.surface.as_ptr()) })
            .ok_or_else(|| "Wayland window has no display".to_string())?;
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));
        // SAFETY: both handles refer to the same connection. on_close/on_quit detach the
        // lease synchronously while the parent still exists.
        unsafe { VideoSurface::attach(player, display, raw, f64::from(scale)) }
            .map_err(|e| e.to_string())
    })?
}

impl NativeVideo {
    pub fn hide(&mut self) {
        if let Some(lease) = &self.lease {
            lease.set_visible(false);
        }
        self.showing = false;
    }

    /// Attaches the GPU surface once a player exists (the first time only). The command
    /// thread asks for this right after creating the player and before its first load, so mpv
    /// isn't reconfigured for the GPU in the middle of loading the first video.
    pub fn try_attach(&mut self, scale: f32) {
        if self.tried {
            return;
        }
        let (Some(window), Some(player)) = (&self.window, crate::video::player_if_started()) else {
            return;
        };
        self.tried = true;
        match attach(window, player, scale) {
            Ok(lease) => {
                eprintln!("nt-rune: Wayland GL video attached");
                self.lease = Some(lease);
            }
            Err(error) => eprintln!("nt-rune: keeping software video: {error}"),
        }
    }

    /// Playback goes through the GPU surface (attached and not fallen back).
    pub fn attached(&self) -> bool {
        self.lease.is_some()
    }

    pub fn detach(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.detach();
        }
        self.showing = false;
    }

    pub fn update(s: &mut App) -> bool {
        if s.native_video.window.is_none() {
            return false;
        }
        let rect = if s.video.active {
            crate::view::player_rect(s).map(|rect| rect.map(|point| point.round() as i32))
        } else {
            None
        };
        let min_y = if s.fullscreen { 0.0 } else { TOP_BAR };
        let scale = s.scale.get();
        let video = &mut s.native_video;
        if rect.is_some() {
            video.try_attach(scale);
        }
        let was_showing = video.showing;
        video.showing = false;
        if let Some(lease) = &video.lease {
            if let Some(error) = lease.error() {
                eprintln!("nt-rune: Wayland video fell back to software: {error}");
                video.detach();
                return was_showing;
            }
            let limits = [
                0,
                min_y.ceil() as i32,
                s.win_w.floor() as i32,
                s.win_h.floor() as i32,
            ];
            let visible = rect.and_then(|rect| {
                let [x, y, w, h] = rect;
                let [left, top, right, bottom] = intersect(rect, limits)?;
                let crop = (
                    (left - x) as u32,
                    (top - y) as u32,
                    (right - left) as u32,
                    (bottom - top) as u32,
                );
                lease.set_rect(x, y, w as u32, h as u32);
                lease.set_clip((crop != (0, 0, w as u32, h as u32)).then_some(crop));
                lease.set_scale(f64::from(scale));
                Some([left as f32, top as f32, right as f32, bottom as f32])
            });
            // Visible while loading too: the parent paints the poster over the child until the
            // hole opens, and a child that is already visible presents mpv's first decoded
            // frame at once instead of after the UI thread has seen Ready (up to a frame later).
            lease.set_visible(visible.is_some());
            // `is_presenting` stays true from the previous video until the worker unmaps it,
            // and the first render after mapping may come before a decoded picture: open the
            // hole only once a frame was rendered after this video's load.
            let ready = nt_core::policy::gpu_picture_ready(
                &s.video.shown.status,
                lease.is_presenting(),
                crate::video::gpu_frames(s.video.serial),
            );
            if let Some([left, top, right, bottom]) = visible.filter(|_| ready) {
                video.showing = true;
                let rects = [
                    [0.0, 0.0, s.win_w, top],
                    [0.0, top, left, bottom - top],
                    [right, top, s.win_w - right, bottom - top],
                    [0.0, bottom, s.win_w, s.win_h - bottom],
                ];
                for (mask, rect) in video.masks.iter().zip(rects) {
                    mask.set(rect);
                }
            }
        }
        // NT_GPU_TRACE=1: one line per update (switch diagnostics), else per hole change.
        static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *TRACE.get_or_init(|| std::env::var_os("NT_GPU_TRACE").is_some());
        if (trace || (was_showing != video.showing && crate::probe::enabled()))
            && let Some(p) = crate::video::player_if_started()
        {
            {
                let st = p.state();
                eprintln!(
                    "GPUTRACE t={:.3} showing={} frames={} pos={:.2} status={:?} src={:?}",
                    crate::scroller::now(),
                    video.showing,
                    p.render_stats().frames,
                    st.position,
                    st.status,
                    st.source
                );
            }
        }
        was_showing != video.showing
    }

    pub fn backdrop(&self, fullscreen: bool) -> El {
        let color = if fullscreen { Color::BLACK } else { t().bg };
        let mut bg = el()
            .key("window-masks")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .inert();
        for mask in &self.masks {
            // Paint only: the masks sit under the page and must never take the pointer.
            bg = bg.child(el().absolute(0.0, 0.0).fill(color).live_rect(mask));
        }
        bg
    }
}

/// Intersect a destination rectangle with the window's video area. Use the same
/// logical pixel edges for the Wayland crop and the transparent hole.
fn intersect([x, y, w, h]: [i32; 4], [min_x, min_y, max_x, max_y]: [i32; 4]) -> Option<[i32; 4]> {
    let left = x.max(min_x);
    let top = y.max(min_y);
    let right = (x + w).min(max_x);
    let bottom = (y + h).min(max_y);
    (right > left && bottom > top).then_some([left, top, right, bottom])
}

#[cfg(test)]
mod tests {
    use super::intersect;

    #[test]
    fn video_area_clips_at_the_toolbar_and_window_edges() {
        let area = [0, 56, 1280, 800];
        assert_eq!(
            intersect([24, 80, 900, 506], area),
            Some([24, 80, 924, 586])
        );
        assert_eq!(intersect([-12, 24, 1300, 900], area), Some(area));
        assert_eq!(intersect([24, -450, 900, 506], area), None);
        assert_eq!(intersect([1280, 80, 900, 506], area), None);
    }
}
