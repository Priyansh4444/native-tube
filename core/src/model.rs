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
    /// 320x180 JPEG, always 16:9.
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
    pub id: String,
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
}

impl WatchInfo {
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
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    /// "@handle".
    pub author: String,
    pub author_channel_id: Option<String>,
    pub author_avatar: Option<String>,
    pub text: String,
    /// "7 years ago".
    pub published: Option<String>,
    /// "71K".
    pub likes: Option<String>,
    /// "687".
    pub reply_count: Option<String>,
    pub by_creator: bool,
}

/// A page of comments, plus the total when the first page carries it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommentPage {
    pub items: Vec<Comment>,
    pub next: Option<Continuation>,
    /// "2,462,007", only on the first page.
    pub total: Option<String>,
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
