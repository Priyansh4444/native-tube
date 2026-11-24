//! Plain data types shared by every frontend.

use serde::{Deserialize, Serialize};

/// A video as shown on a card or in a list row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    /// Channel display name. Empty when the source list doesn't carry one.
    pub channel: String,
    pub channel_id: Option<String>,
    /// Small round avatar URL (JPEG), when the source list carries one.
    pub channel_avatar: Option<String>,
    /// Video poster URL. Pass it to `images::image` with the drawn width in physical pixels.
    pub thumbnail: String,
    /// "12:34", or `None` for live streams.
    pub duration: Option<String>,
    /// "1.3M views" / "4.2K watching".
    pub views: Option<String>,
    /// "3 years ago".
    pub published: Option<String>,
    pub is_live: bool,
    /// A line or two of description, when search provides it.
    pub snippet: Option<String>,
    #[serde(default)]
    pub moment: Option<Moment>,
    /// Card badges as YouTube labels them: "4K", "CC", "New", "8K", "LIVE"…
    #[serde(default)]
    pub badges: Vec<String>,
    /// The channel carries a verified (or official artist) mark.
    #[serde(default)]
    pub channel_verified: bool,
    /// For a scheduled premiere or stream: "Premieres in 2 days", "Scheduled for 11/20/26".
    #[serde(default)]
    pub upcoming: Option<String>,
}

/// A filter chip above a list ("All", "From Fireship", "Related", "Recently uploaded").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chip {
    pub label: String,
    /// The chip whose list is showing.
    pub selected: bool,
    /// Pass to [`crate::Client::more_videos`] for the chip's first page. `None` for the list
    /// already showing.
    pub next: Option<Continuation>,
}

/// A matched line in a transcript. Ranges are UTF-8 byte offsets in `snippet`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    pub start_secs: f64,
    pub label: String,
    pub snippet: String,
    pub highlights: Vec<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchNotice {
    AgeGated { title: String, body: String },
    NoResults,
    Corrected { showing: String, original: String },
    DidYouMean { suggestion: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub results: Page<Video>,
    pub notices: Vec<SearchNotice>,
    pub transcripts: Page<Video>,
    pub transcript_total: Option<u64>,
    #[serde(default)]
    pub chips: Vec<SearchOption>,
    #[serde(default)]
    pub filters: Vec<SearchFilterGroup>,
    #[serde(default)]
    pub entries: Vec<SearchResult>,
}

/// A query modifier returned by YouTube or a request for transcript search.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchFilter {
    #[default]
    All,
    Params(String),
    Chip(Continuation),
    Transcripts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchOption {
    pub label: String,
    pub selected: bool,
    /// None when YouTube disabled the option.
    pub filter: Option<SearchFilter>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilterGroup {
    pub title: String,
    pub options: Vec<SearchOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SearchResult {
    Video(Box<Video>),
    Playlist(PlaylistRef),
    Channel(SearchChannel),
    Shelf(SearchShelf),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistRef {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub video_count: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchChannel {
    pub channel: ChannelRef,
    pub handle: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchShelf {
    pub title: String,
    pub items: Vec<SearchResult>,
}

impl Video {
    /// The canonical 320x180 thumbnail URL for a video id.
    pub fn thumbnail_url(id: &str) -> String {
        format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
    }

    /// "Channel · 1.3M views · 3 years ago", skipping missing parts.
    pub fn meta_line(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if let Some(v) = self.views.as_deref() {
            parts.push(v);
        }
        if let Some(p) = self.published.as_deref() {
            parts.push(p);
        }
        parts.join(" • ")
    }
}

/// A reference to a channel: enough to show a row and to subscribe.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    /// "4.56M subscribers".
    pub subscribers: Option<String>,
}

/// Where a list continues from. Opaque to frontends: pass it back to
/// [`crate::Client::more_videos`] or [`crate::Client::more_comments`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Continuation {
    pub(crate) endpoint: String,
    pub(crate) token: String,
    /// For a channel's uploads: the channel, since later pages don't name it.
    #[serde(default)]
    pub(crate) owner: Option<(String, String, Option<String>)>,
}

/// One page of a paginated list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` when the list has ended.
    pub next: Option<Continuation>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Page {
            items: Vec::new(),
            next: None,
        }
    }
}

/// Everything the watch page shows besides the player and the comments.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchInfo {
    pub(crate) id: String,
    pub title: String,
    /// "1,836,226,895 views".
    pub views: Option<String>,
    /// "Oct 24, 2009".
    pub date: Option<String>,
    /// "19M".
    pub likes: Option<String>,
    pub description: String,
    pub channel: ChannelRef,
    pub related: Page<Video>,
    /// Pass to [`crate::Client::more_comments`] to load the first page of comments.
    pub comments: Option<Continuation>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
    #[serde(default)]
    pub heatmap: Vec<HeatPoint>,
    /// The channel carries a verified (or official artist) mark.
    #[serde(default)]
    pub channel_verified: bool,
    /// Chips above the related list; empty when YouTube sends none.
    #[serde(default)]
    pub related_chips: Vec<Chip>,
    pub playlist: Option<PlaylistPanel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// Start time in seconds.
    pub start: f64,
    pub title: String,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatPoint {
    /// Position and span in seconds.
    pub start: f64,
    pub duration: f64,
    /// Normalized height in the range 0..=1.
    pub intensity: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaylistPanel {
    pub id: String,
    pub title: String,
    pub items: Vec<Video>,
    pub index: Option<usize>,
    pub next: Option<Continuation>,
    pub is_mix: bool,
}

impl PlaylistPanel {
    pub fn next_video(&self) -> Option<&Video> {
        self.items.get(self.index?.checked_add(1)?)
    }

    pub fn previous_video(&self) -> Option<&Video> {
        self.items.get(self.index?.checked_sub(1)?)
    }
}

impl WatchInfo {
    pub fn chapter_at(&self, seconds: f64) -> Option<&Chapter> {
        let end = self
            .chapters
            .partition_point(|chapter| chapter.start <= seconds);
        self.chapters.get(end.checked_sub(1)?)
    }

    /// The video as a card, for history and for lists.
    pub fn as_video(&self) -> Video {
        Video {
            id: self.id.clone(),
            title: self.title.clone(),
            channel: self.channel.name.clone(),
            channel_id: Some(self.channel.id.clone()).filter(|s| !s.is_empty()),
            channel_avatar: self.channel.avatar.clone(),
            thumbnail: Video::thumbnail_url(&self.id),
            duration: None,
            views: self.views.clone(),
            published: self.date.clone(),
            is_live: false,
            snippet: None,
            moment: None,
            channel_verified: self.channel_verified,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    /// "@handle".
    pub author: String,
    pub(crate) author_channel_id: Option<String>,
    pub author_avatar: Option<String>,
    pub text: String,
    /// "7 years ago".
    pub published: Option<String>,
    /// "71K".
    pub likes: Option<String>,
    /// "687".
    pub reply_count: Option<String>,
    pub by_creator: bool,
    /// Pass to [`crate::Client::more_replies`] to load this comment's replies; `None` without any.
    #[serde(default)]
    pub replies: Option<Continuation>,
    /// Pinned to the top by the channel ("Pinned by @handle").
    #[serde(default)]
    pub pinned: bool,
    /// The creator gave this comment a heart.
    #[serde(default)]
    pub hearted: bool,
    /// The author's channel is verified.
    #[serde(default)]
    pub author_verified: bool,
}

/// A comment sort order ("Top comments", "Newest first"), from the first page's header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommentSort {
    pub label: String,
    /// The order the page was loaded in.
    pub selected: bool,
    /// Pass to [`crate::Client::more_comments`] to load the first page in this order.
    pub next: Continuation,
}

/// A page of comments, plus the total when the first page carries it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommentPage {
    pub items: Vec<Comment>,
    pub next: Option<Continuation>,
    /// "2,462,007", only on the first page.
    pub total: Option<String>,
    /// Sort orders, only on the first page of top-level comments.
    #[serde(default)]
    pub sort: Vec<CommentSort>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    /// "@Fireship".
    pub handle: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    /// "4.33M subscribers".
    pub subscribers: Option<String>,
    /// "863 videos".
    pub video_count: Option<String>,
    pub description: String,
    pub videos: Page<Video>,
    /// The channel carries a verified (or official artist) mark.
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub tabs: Vec<ChannelTab>,
    #[serde(default)]
    pub selected_tab: Option<ChannelTab>,
    #[serde(default)]
    pub video_sort: Vec<Chip>,
    #[serde(default)]
    pub playlists: Page<PlaylistRef>,
    #[serde(default)]
    pub featured: Option<Video>,
    pub links_label: Option<String>,
    #[serde(default)]
    pub links: Vec<ChannelLink>,
    pub about: Option<Continuation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelTab {
    Home,
    Videos,
    Live,
    Playlists,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelLink {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAbout {
    pub description: String,
    pub links: Vec<ChannelLink>,
}

impl ChannelInfo {
    pub fn as_ref(&self) -> ChannelRef {
        ChannelRef {
            id: self.id.clone(),
            name: self.name.clone(),
            avatar: self.avatar.clone(),
            subscribers: self.subscribers.clone(),
        }
    }
}

/// The initial Home feed and the chips returned alongside it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HomeFeed {
    pub videos: Page<Video>,
    pub chips: Vec<Chip>,
}

/// A home feed category.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    pub id: &'static str,
    pub title: &'static str,
    #[serde(skip)]
    pub(crate) query: &'static str,
}

/// The home feed's categories, in display order. The first is the default.
pub const TOPICS: &[Topic] = &[
    Topic {
        id: "all",
        title: "All",
        query: "",
    },
    Topic {
        id: "music",
        title: "Music",
        query: "music videos",
    },
    Topic {
        id: "gaming",
        title: "Gaming",
        query: "gaming",
    },
    Topic {
        id: "news",
        title: "News",
        query: "news today",
    },
    Topic {
        id: "tech",
        title: "Tech",
        query: "technology",
    },
    Topic {
        id: "programming",
        title: "Programming",
        query: "programming",
    },
    Topic {
        id: "science",
        title: "Science",
        query: "science documentary",
    },
    Topic {
        id: "sports",
        title: "Sports",
        query: "sports highlights",
    },
    Topic {
        id: "podcasts",
        title: "Podcasts",
        query: "podcast full episode",
    },
    Topic {
        id: "cooking",
        title: "Cooking",
        query: "cooking recipes",
    },
];

/// Decoded straight-alpha RGBA8 pixels, tightly packed.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: bytes::Bytes,
}
