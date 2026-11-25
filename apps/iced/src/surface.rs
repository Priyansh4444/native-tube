//! An optional Wayland child surface. Its position is committed with the same root draw
//! that paints the alpha hole, including scroll-only frames that never rebuild the view.
#[cfg(target_os = "linux")]
use crate::app::Body;
use crate::app::{App, Message};
use iced::advanced::Renderer as _;
use iced::advanced::renderer::Quad;
use iced::{Color, Rectangle, Renderer, Task, window};

pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        cfg!(target_os = "linux")
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var_os("NT_SOFTWARE_VIDEO").is_none()
    })
}

#[cfg(target_os = "linux")]
static LEASE: std::sync::Mutex<Option<nt_core::VideoSurface>> = std::sync::Mutex::new(None);

pub fn attach(id: window::Id, scale: f32) -> Task<Message> {
    #[cfg(target_os = "linux")]
    if enabled() {
        return window::run(id, move |window| {
            let Some(player) = crate::ext::existing_player() else {
                return;
            };
            if player.state().source.is_some() {
                return;
            }
            let Ok(display) = window.display_handle() else {
                return;
            };
            let Ok(handle) = window.window_handle() else {
                return;
            };
            // Iced runs this callback on its window thread. Quit detaches before iced::exit.
            match unsafe {
                nt_core::VideoSurface::attach(
                    player,
                    display.as_raw(),
                    handle.as_raw(),
                    f64::from(scale),
                )
            } {
                Ok(lease) => {
                    lease.set_visible(false);
                    *LEASE.lock().unwrap_or_else(|e| e.into_inner()) = Some(lease);
                    crate::scroller::trace_event("gpu_surface_attached");
                }
                Err(error) => eprintln!("nt-iced: software video fallback: {error}"),
            }
        })
        .map(|()| Message::SurfaceAttached);
    }
    let _ = (id, scale);
    Task::none()
}

pub fn detach() {
    #[cfg(target_os = "linux")]
    {
        let lease = LEASE.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(surface) = lease {
            surface.detach();
        }
    }
}

pub fn draw(app: &App, renderer: &mut Renderer, bounds: Rectangle) {
    let hole = update_surface(app, bounds);
    app.surface_visible.set(hole.is_some());
    paint_around(renderer, bounds, hole, crate::theme::BG);
}

#[cfg(target_os = "linux")]
fn update_surface(app: &App, window: Rectangle) -> Option<Rectangle> {
    let lease = LEASE.lock().unwrap_or_else(|e| e.into_inner());
    let surface = lease.as_ref()?;
    let Some(page) = app.top().filter(|p| Some(p.id) == app.playing_page) else {
        surface.set_visible(false);
        return None;
    };
    let Body::Watch(watch) = &page.body else {
        surface.set_visible(false);
        return None;
    };
    let geo = crate::view::watch_geo(app);
    let top = if app.fullscreen {
        0.0
    } else {
        crate::theme::TOP_BAR
    };
    let origin = if app.fullscreen || geo.theatre {
        0.0
    } else {
        24.0
    };
    let offset = if app.fullscreen {
        0.0
    } else {
        page.scroll.offset()
    };
    let (width, height) = fit(
        geo.player_w,
        geo.player_h,
        app.play.aspect().unwrap_or(16.0 / 9.0),
    );
    let x = if app.fullscreen || geo.theatre {
        0.0
    } else {
        geo.px
    };
    let rect = Rectangle {
        x: (x + (geo.player_w - width) / 2.0).round(),
        y: (top + origin - offset + (geo.player_h - height) / 2.0).round(),
        width,
        height,
    };
    let clip = Rectangle {
        y: top,
        height: (window.height - top).max(0.0),
        ..window
    };
    // A Wayland subsurface can extend outside its parent, and the parent can't mask pixels
    // outside its own bounds. So the child is cropped to the part of the picture inside the
    // scroll area (below the top bar): the visible part keeps playing while the player
    // scrolls under the bar.
    let visible = rect
        .intersection(&clip)
        .filter(|v| v.width >= 1.0 && v.height >= 1.0);
    surface.set_scale(f64::from(app.scale));
    surface.set_visible(visible.is_some());
    let visible = visible?;
    surface.set_rect(rect.x as i32, rect.y as i32, width as u32, height as u32);
    let crop = (
        (visible.x - rect.x).round().max(0.0) as u32,
        (visible.y - rect.y).round().max(0.0) as u32,
        visible.width.round() as u32,
        visible.height.round() as u32,
    );
    surface.set_clip((crop != (0, 0, width as u32, height as u32)).then_some(crop));
    // Only a real decoded frame of this load counts (`presented_frames`); the surface also
    // reports presenting after the black render it maps with, which would flash black.
    let frames = crate::ext::existing_player().map_or(0, |p| p.presented_frames());
    if !nt_core::policy::gpu_picture_ready(&app.play.status, surface.is_presenting(), frames) {
        return None;
    }
    watch.first_frame.set(true);
    if app.want_playback_mark.replace(false) {
        crate::scroller::mark_on_next_frame(nt_core::metrics::marks::PLAYBACK_STARTED);
    }
    Some(visible)
}

#[cfg(not(target_os = "linux"))]
fn update_surface(_: &App, _: Rectangle) -> Option<Rectangle> {
    None
}

pub fn fit(width: f32, height: f32, aspect: f32) -> (f32, f32) {
    let aspect = aspect.clamp(0.2, 5.0);
    let w = width.min(height * aspect).floor().max(1.0);
    (w, (w / aspect).min(height).floor().max(1.0))
}

fn paint_around(renderer: &mut Renderer, bounds: Rectangle, hole: Option<Rectangle>, bg: Color) {
    let mut paint = |bounds: Rectangle| {
        if bounds.width > 0.0 && bounds.height > 0.0 {
            renderer.fill_quad(
                Quad {
                    bounds,
                    ..Quad::default()
                },
                bg,
            );
        }
    };
    if let Some(hole) = hole {
        let bottom = bounds.y + bounds.height;
        let right = bounds.x + bounds.width;
        paint(Rectangle {
            height: hole.y - bounds.y,
            ..bounds
        });
        paint(Rectangle {
            y: hole.y + hole.height,
            height: bottom - hole.y - hole.height,
            ..bounds
        });
        paint(Rectangle {
            y: hole.y,
            height: hole.height,
            width: hole.x - bounds.x,
            ..bounds
        });
        paint(Rectangle {
            x: hole.x + hole.width,
            y: hole.y,
            height: hole.height,
            width: right - hole.x - hole.width,
        });
    } else {
        paint(bounds);
    }
}
