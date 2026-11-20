//! The Wayland child surface holds video below GPUI's transparent parent.
use gpui::{Bounds, Hsla, Pixels, Window};

/// Paint the parent around the Wayland video. Adapted from Akshar Patel's yt-music-gpui
/// player_page.rs (MIT/Apache-2.0).
pub fn paint_around(
    bounds: Bounds<Pixels>,
    hole: Bounds<Pixels>,
    color: Hsla,
    radius: f32,
    window: &mut Window,
) {
    let original = hole;
    let hole = bounds.intersect(&hole);
    let point = gpui::point;
    for (a, b) in [
        (bounds.origin, point(bounds.right(), hole.top())),
        (point(bounds.left(), hole.bottom()), bounds.bottom_right()),
        (
            point(bounds.left(), hole.top()),
            point(hole.left(), hole.bottom()),
        ),
        (
            point(hole.right(), hole.top()),
            point(bounds.right(), hole.bottom()),
        ),
    ] {
        if b.x > a.x && b.y > a.y {
            window.paint_quad(gpui::fill(Bounds::from_corners(a, b), color));
        }
    }
    if radius <= 0.0 {
        return;
    }
    // Fill just the corners that are outside the same 12 px round used by the poster.
    const CIRCLE: f32 = 0.552_285;
    for (corner, dx, dy) in [
        (original.origin, 1.0, 1.0),
        (point(original.right(), original.top()), -1.0, 1.0),
        (original.bottom_right(), -1.0, -1.0),
        (point(original.left(), original.bottom()), 1.0, -1.0),
    ] {
        let at = |x, y| {
            point(
                corner.x + gpui::px(x * radius * dx),
                corner.y + gpui::px(y * radius * dy),
            )
        };
        let mut path = gpui::PathBuilder::fill();
        path.move_to(corner);
        path.line_to(at(1.0, 0.0));
        path.cubic_bezier_to(at(0.0, 1.0), at(1.0 - CIRCLE, 0.0), at(0.0, 1.0 - CIRCLE));
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

#[cfg(target_os = "linux")]
pub struct Surface {
    inner: nt_core::VideoSurface,
    last_rect: Option<Bounds<Pixels>>,
    last_clip: Option<(u32, u32, u32, u32)>,
    scale: f32,
}

#[cfg(target_os = "linux")]
impl Surface {
    pub fn attach(player: &nt_core::Player, window: &Window) -> nt_core::Result<Self> {
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let display = window
            .display_handle()
            .map_err(|e| nt_core::Error::Player(e.to_string()))?;
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|e| nt_core::Error::Player(e.to_string()))?;
        let scale = window.scale_factor();
        // SAFETY: GPUI supplied both handles for this window on its owning thread. The
        // window's close callback and the app's quit callback detach the lease first.
        let inner = unsafe {
            nt_core::VideoSurface::attach(
                player,
                display.as_raw(),
                handle.as_raw(),
                f64::from(scale),
            )?
        };
        window.set_background_appearance(gpui::WindowBackgroundAppearance::Transparent);
        Ok(Self {
            inner,
            last_rect: None,
            last_clip: None,
            scale,
        })
    }

    /// `rect` is the whole player box, `area` the part of the window the page may show it in
    /// (below the top bar, or the whole window in fullscreen). A box partly scrolled out keeps
    /// its size and position; core's `set_clip` shows only the visible part, so the child never
    /// spills past the window. A box with nothing visible hides the child.
    pub fn update(&mut self, rect: Option<Bounds<Pixels>>, area: Bounds<Pixels>, scale: f32) {
        let visible = rect.map(|r| (r, r.intersect(&area)));
        let rect = visible
            .filter(|(_, v)| v.size.width > gpui::px(0.0) && v.size.height > gpui::px(0.0))
            .map(|(r, _)| r);
        let clip = visible.and_then(|(r, v)| {
            (rect.is_some() && v != r).then(|| {
                let n = |p: Pixels| f32::from(p).round().max(0.0) as u32;
                (
                    n(v.origin.x - r.origin.x),
                    n(v.origin.y - r.origin.y),
                    n(v.size.width),
                    n(v.size.height),
                )
            })
        });
        if self.last_clip != clip {
            self.inner.set_clip(clip);
            self.last_clip = clip;
        }
        if self.last_rect != rect {
            if let Some(rect) = rect {
                self.inner.set_rect(
                    f32::from(rect.origin.x).round() as i32,
                    f32::from(rect.origin.y).round() as i32,
                    f32::from(rect.size.width).round() as u32,
                    f32::from(rect.size.height).round() as u32,
                );
            }
            if self.last_rect.is_some() != rect.is_some() {
                self.inner.set_visible(rect.is_some());
            }
            self.last_rect = rect;
        }
        if self.scale != scale {
            self.inner.set_scale(f64::from(scale));
            self.scale = scale;
        }
    }

    pub fn is_presenting(&self) -> bool {
        self.last_rect.is_some() && self.inner.is_presenting()
    }

    pub fn error(&self) -> Option<String> {
        self.inner.error()
    }

    pub fn detach(self) {
        self.inner.detach();
    }
}
