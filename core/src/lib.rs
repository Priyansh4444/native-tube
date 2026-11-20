//! nt-core: everything the NativeTube clients share that isn't UI.
//!
//! - [`client()`]: YouTube data (home, search, watch, comments, channels,
//!   suggestions, subscription feeds) as runtime-agnostic [`Job`]s.
//! - [`images::image`]: thumbnails and avatars, decoded and downscaled off the UI thread.
//! - [`library()`]: subscriptions, history and Watch Later, saved locally.
//! - [`Player`]: libmpv playback that hands out RGBA/BGRA frames.
//! - [`scroll`]: Chromium's scroll physics, so every frontend scrolls the same way.
//! - [`metrics`] and [`autopilot`]: the benchmark harness's hooks.

pub mod autopilot;
mod captions;
mod client;
mod error;
mod fixtures;
pub mod format;
pub mod images;
mod innertube;
mod library;
pub mod metrics;
mod model;
#[cfg(target_os = "linux")]
mod mpris;
pub mod player;
mod polite;
pub mod resolve;
pub mod scroll;
mod storyboard;
mod transcripts;

pub use captions::{CaptionLanguage, CaptionTrack};
pub use client::{Client, Job, client, runtime};
pub use error::{Error, Result};
pub use library::{LastPlayed, Library, LibraryData, data_dir, library};
pub use model::*;
#[cfg(target_os = "linux")]
pub use player::VideoSurface;
pub use player::{PixelFormat, PlaybackState, Player, Status, VideoFrame};
pub use resolve::AudioTrack;
pub use storyboard::{Storyboard, StoryboardFrame, StoryboardLevel};

#[cfg(test)]
mod proptests;
