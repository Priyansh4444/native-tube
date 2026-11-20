//! The shared icon set (Material Symbols Outlined, 24 px, from `assets/icons`), embedded in the
//! binary. The paths are black; every use tints them with a colour.

use iced::widget::svg::{self, Handle};
use iced::{Color, Element, Length};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    ArrowBack,
    Search,
    Home,
    HomeFill,
    Subscriptions,
    SubscriptionsFill,
    History,
    HistoryFill,
    Schedule,
    ScheduleFill,
    MoreVert,
    PlayArrow,
    Pause,
    Replay,
    SkipNext,
    VolumeUp,
    VolumeDown,
    VolumeOff,
    Settings,
    Fullscreen,
    FullscreenExit,
    WidthWide,
    ThumbUp,
    ThumbDown,
    Share,
    Check,
    Close,
    Notifications,
    ChevronLeft,
    ChevronRight,
    Error,
    WifiOff,
    VideoLibrary,
    Delete,
    Speed,
    Tune,
    Menu,
    ClosedCaption,
    ClosedCaptionFill,
    ExpandMore,
    ExpandLess,
}

const COUNT: usize = Icon::ExpandLess as usize + 1;

impl Icon {
    fn index(self) -> usize {
        self as usize
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Icon::ArrowBack => include_bytes!("../../../assets/icons/arrow_back.svg"),
            Icon::Search => include_bytes!("../../../assets/icons/search.svg"),
            Icon::Home => include_bytes!("../../../assets/icons/home.svg"),
            Icon::HomeFill => include_bytes!("../../../assets/icons/home_fill1.svg"),
            Icon::Subscriptions => include_bytes!("../../../assets/icons/subscriptions.svg"),
            Icon::SubscriptionsFill => {
                include_bytes!("../../../assets/icons/subscriptions_fill1.svg")
            }
            Icon::History => include_bytes!("../../../assets/icons/history.svg"),
            Icon::HistoryFill => include_bytes!("../../../assets/icons/history_fill1.svg"),
            Icon::Schedule => include_bytes!("../../../assets/icons/schedule.svg"),
            Icon::ScheduleFill => include_bytes!("../../../assets/icons/schedule_fill1.svg"),
            Icon::MoreVert => include_bytes!("../../../assets/icons/more_vert.svg"),
            Icon::PlayArrow => include_bytes!("../../../assets/icons/play_arrow.svg"),
            Icon::Pause => include_bytes!("../../../assets/icons/pause.svg"),
            Icon::Replay => include_bytes!("../../../assets/icons/replay.svg"),
            Icon::SkipNext => include_bytes!("../../../assets/icons/skip_next.svg"),
            Icon::VolumeUp => include_bytes!("../../../assets/icons/volume_up.svg"),
            Icon::VolumeDown => include_bytes!("../../../assets/icons/volume_down.svg"),
            Icon::VolumeOff => include_bytes!("../../../assets/icons/volume_off.svg"),
            Icon::Settings => include_bytes!("../../../assets/icons/settings.svg"),
            Icon::Fullscreen => include_bytes!("../../../assets/icons/fullscreen.svg"),
            Icon::FullscreenExit => include_bytes!("../../../assets/icons/fullscreen_exit.svg"),
            Icon::WidthWide => include_bytes!("../../../assets/icons/width_wide.svg"),
            Icon::ThumbUp => include_bytes!("../../../assets/icons/thumb_up.svg"),
            Icon::ThumbDown => include_bytes!("../../../assets/icons/thumb_down.svg"),
            Icon::Share => include_bytes!("../../../assets/icons/share.svg"),
            Icon::Check => include_bytes!("../../../assets/icons/check.svg"),
            Icon::Close => include_bytes!("../../../assets/icons/close.svg"),
            Icon::Notifications => include_bytes!("../../../assets/icons/notifications.svg"),
            Icon::ChevronLeft => include_bytes!("../../../assets/icons/chevron_left.svg"),
            Icon::ChevronRight => include_bytes!("../../../assets/icons/chevron_right.svg"),
            Icon::Error => include_bytes!("../../../assets/icons/error.svg"),
            Icon::WifiOff => include_bytes!("../../../assets/icons/wifi_off.svg"),
            Icon::VideoLibrary => include_bytes!("../../../assets/icons/video_library.svg"),
            Icon::Delete => include_bytes!("../../../assets/icons/delete.svg"),
            Icon::Speed => include_bytes!("../../../assets/icons/speed.svg"),
            Icon::Tune => include_bytes!("../../../assets/icons/tune.svg"),
            Icon::ClosedCaption => include_bytes!("../../../assets/icons/closed_caption.svg"),
            Icon::ClosedCaptionFill => {
                include_bytes!("../../../assets/icons/closed_caption_fill1.svg")
            }
            Icon::ExpandMore => include_bytes!("../../../assets/icons/expand_more.svg"),
            Icon::ExpandLess => include_bytes!("../../../assets/icons/expand_less.svg"),
            Icon::Menu => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 6h18v2H3zm0 5h18v2H3zm0 5h18v2H3z"/></svg>"#,
        }
    }

    /// The embedded SVG, parsed once.
    pub fn handle(self) -> Handle {
        static HANDLES: [OnceLock<Handle>; COUNT] = [const { OnceLock::new() }; COUNT];
        HANDLES
            .get(self.index())
            .map(|slot| {
                slot.get_or_init(|| Handle::from_memory(self.bytes()))
                    .clone()
            })
            .unwrap_or_else(|| Handle::from_memory(self.bytes()))
    }
}

/// An icon element `size` px square, tinted `color`.
pub fn icon<'a, M: 'a>(i: Icon, size: f32, color: Color) -> Element<'a, M> {
    svg(i, size, color).into()
}

pub fn svg(i: Icon, size: f32, color: Color) -> svg::Svg<'static> {
    svg::Svg::new(i.handle())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_, _| svg::Style { color: Some(color) })
}
