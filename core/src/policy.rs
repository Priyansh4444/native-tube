//! Shared app policies, so the three frontends do the same work at the same moments.
//!
//! - [`Prewarm`]: what to start ahead of the first click (`NT_PREWARM=off|player|full`), and
//!   [`PREWARM_AFTER_HOME`], when every frontend starts it.
//! - [`comments_due`]: when a watch page asks for its first page of comments.
//! - [`gpu_picture_ready`]: when the GPU path may cut the video hole (and mark the first frame).
//!
//! The `playback_started` mark has one definition: the first frame of the current load that
//! the app has *presented* with the picture, i.e. the frame that cuts the hole on the GPU path
//! ([`gpu_picture_ready`]) or that draws the uploaded picture on the software path. Every
//! frontend records it, and every other mark tied to a frame, right after that frame's GPU
//! submit and present call returns (iced: the first update after `present`; GPUI: a deferred
//! effect after `Window::present`; Rune: `on_frame_encoded` / `on_frame_drawn`).

use crate::Status;
use std::sync::OnceLock;
use std::time::Duration;

/// What a frontend starts while the user looks at Home, before the first video is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prewarm {
    /// Nothing until the first open: it creates libmpv and attaches the GPU surface.
    Off,
    /// Create libmpv after Home has drawn. The GPU surface attaches on the first open.
    Player,
    /// Create libmpv and attach the GPU video surface after Home has drawn.
    Full,
}

/// How long after Home has drawn (the frame carrying `home_loaded`) a prewarm starts.
pub const PREWARM_AFTER_HOME: Duration = Duration::from_millis(250);

impl Prewarm {
    /// The policy for this process: `NT_NO_PREWARM` (any value) means [`Prewarm::Off`]; otherwise
    /// `NT_PREWARM` picks one; otherwise the frontend's shipped `default`. Read once.
    pub fn for_process(default: Prewarm) -> Prewarm {
        static POLICY: OnceLock<Prewarm> = OnceLock::new();
        *POLICY.get_or_init(|| {
            let requested = std::env::var("NT_PREWARM").ok();
            let policy = Self::parse(
                std::env::var_os("NT_NO_PREWARM").is_some(),
                requested.as_deref(),
                default,
            );
            if let Some(value) =
                requested.filter(|v| !matches!(v.trim(), "off" | "player" | "full"))
            {
                eprintln!(
                    "nt-core: NT_PREWARM={value:?} ignored; prewarm {}",
                    policy.name()
                );
            }
            trace(&format!("policy {}", policy.name()));
            policy
        })
    }

    /// `no_prewarm` wins; an unknown `value` keeps `default`.
    fn parse(no_prewarm: bool, value: Option<&str>, default: Prewarm) -> Prewarm {
        if no_prewarm {
            return Prewarm::Off;
        }
        match value.map(str::trim) {
            Some("off") => Prewarm::Off,
            Some("player") => Prewarm::Player,
            Some("full") => Prewarm::Full,
            _ => default,
        }
    }

    /// Whether libmpv starts ahead of the first click.
    pub fn player(self) -> bool {
        self != Prewarm::Off
    }

    /// Whether the GPU video surface attaches ahead of the first click.
    pub fn surface(self) -> bool {
        self == Prewarm::Full
    }

    fn name(self) -> &'static str {
        match self {
            Prewarm::Off => "off",
            Prewarm::Player => "player",
            Prewarm::Full => "full",
        }
    }
}

/// With `NT_TRACE` or `NT_AUTOPILOT`, log a prewarm step (`player`, `surface`) with its time, so
/// a benchmark can check that every frontend starts it at the same point.
pub fn trace(step: &str) {
    if std::env::var_os("NT_TRACE").is_some() || std::env::var_os("NT_AUTOPILOT").is_some() {
        eprintln!("NT_PREWARM {step} {:.1}", crate::metrics::now() * 1000.0);
    }
}

/// How far below the viewport's bottom edge, in viewport heights, the comments header may be
/// when the first page of comments is requested (YouTube's behaviour: lazily, with lead time).
const COMMENTS_LEAD_VIEWPORTS: f64 = 1.0;

/// Whether a watch page should ask for its first page of comments now: the comments header's
/// top (`header_top`, in content coordinates) is within [`COMMENTS_LEAD_VIEWPORTS`] of the
/// viewport's bottom edge. `viewport_top` is the scroll offset the motion is heading to.
/// Frontends check it after every layout and every scroll, not only on scroll.
pub fn comments_due(header_top: f64, viewport_top: f64, viewport_h: f64) -> bool {
    header_top.is_finite()
        && viewport_h > 0.0
        && header_top <= viewport_top + viewport_h * (1.0 + COMMENTS_LEAD_VIEWPORTS)
}

/// The GPU path's first picture of the current load: the player is ready (or ended), the
/// surface is presenting and core counted a decoded frame on it since this load
/// ([`crate::Player::presented_frames`] went 0 → 1). Until then the app keeps its poster up;
/// the frame that cuts the hole is the one that carries `playback_started`.
pub fn gpu_picture_ready(status: &Status, presenting: bool, presented_frames: u64) -> bool {
    matches!(status, Status::Ready | Status::Ended) && presenting && presented_frames > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prewarm_parse() {
        use Prewarm::*;
        assert_eq!(Prewarm::parse(false, None, Full), Full);
        assert_eq!(Prewarm::parse(false, None, Off), Off);
        assert_eq!(Prewarm::parse(false, Some("off"), Full), Off);
        assert_eq!(Prewarm::parse(false, Some("player"), Off), Player);
        assert_eq!(Prewarm::parse(false, Some(" full\n"), Off), Full);
        assert_eq!(Prewarm::parse(false, Some("bogus"), Player), Player);
        // NT_NO_PREWARM=1 keeps meaning off, whatever NT_PREWARM says.
        assert_eq!(Prewarm::parse(true, Some("full"), Full), Off);
        assert_eq!(Prewarm::parse(true, None, Full), Off);
    }

    #[test]
    fn prewarm_parts() {
        assert!(!Prewarm::Off.player() && !Prewarm::Off.surface());
        assert!(Prewarm::Player.player() && !Prewarm::Player.surface());
        assert!(Prewarm::Full.player() && Prewarm::Full.surface());
    }

    #[test]
    fn comments_lead() {
        // An 800-pt viewport at the top: the header may be up to 1600 pt down.
        assert!(comments_due(700.0, 0.0, 800.0));
        assert!(comments_due(1600.0, 0.0, 800.0));
        assert!(!comments_due(1600.5, 0.0, 800.0));
        // Scrolling (or a fling heading) down brings it within reach.
        assert!(comments_due(2400.0, 800.0, 800.0));
        // Not laid out yet, or no viewport: never.
        assert!(!comments_due(f64::INFINITY, 0.0, 800.0));
        assert!(!comments_due(100.0, 0.0, 0.0));
    }

    #[test]
    fn gpu_picture() {
        assert!(!gpu_picture_ready(&Status::Ready, true, 0));
        assert!(!gpu_picture_ready(&Status::Loading, true, 3));
        assert!(!gpu_picture_ready(&Status::Ready, false, 3));
        assert!(gpu_picture_ready(&Status::Ready, true, 1));
        assert!(gpu_picture_ready(&Status::Ended, true, 9));
    }
}
