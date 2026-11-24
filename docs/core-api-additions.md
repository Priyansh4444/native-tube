# Core API additions

Call `Client` and `Player` methods from a frontend event handler unless stated otherwise.
Await a `Job<T>` on the frontend executor, use `try_take` during an update, or use
`on_done` and dispatch its result back to the UI. `Job::wait` is for CLI and test threads.
Associate asynchronous results with the page or video that requested them.

## A1. Captions

```rust
pub struct CaptionTrack {
    pub language_code: String,
    pub label: String,
    pub auto_generated: bool,
    pub translatable: bool,
    // Private signed URL.
}
pub struct CaptionLanguage { pub code: String, pub label: String }
impl Player {
    pub fn set_captions(&self, selection: Option<(usize, Option<String>)>) -> Job<()>;
}
// New fields on resolve::Streams and PlaybackState
pub captions: Vec<CaptionTrack>,
pub caption_languages: Vec<CaptionLanguage>,
// PlaybackState only
pub selected_captions: Option<(usize, Option<String>)>,
```

When playback is ready, show a CC button if tracks are available. Use the zero-based track
index from the current state. Offer translation languages only for a translatable track.
The next video starts with captions off. Draw the regular video frame and report errors
from the job. The rendered frame already includes the subtitle overlay.

```rust
let state = player.state();
if let Some(index) = state.captions.iter().position(|c| c.language_code == "en") {
    player.set_captions(Some((index, None))).await?;
}
```

## A2. Chapters and replay heatmap

```rust
pub struct Chapter { pub start: f64, pub title: String, pub thumbnail: Option<String> }
pub struct HeatPoint { pub start: f64, pub duration: f64, pub intensity: f64 }
// New fields on WatchInfo
pub chapters: Vec<Chapter>,
pub heatmap: Vec<HeatPoint>,
impl WatchInfo {
    pub fn chapter_at(&self, seconds: f64) -> Option<&Chapter>;
}
```

Times are seconds. Draw chapter gaps at the sorted start times and seek there when a row is
selected. Load optional thumbnails through `images::image`. Draw the heatmap over the seek
bar with the samples' spans and normalized heights from 0 to 1. An empty list means the
response omitted that feature. All this data comes from the existing Watch request.
The heatmap is read from the `/next` answer's `frameworkUpdates` (a `macroMarkersListEntity`
of type `MARKER_TYPE_HEATMAP` for this video, about 100 samples), falling back to the older
`playerOverlays` renderers; logged-out answers carry it for most popular videos. No app change
is needed: `WatchInfo::heatmap` simply stops being empty.

```rust
let watch = client().watch(video_id).await?;
let current = watch.chapter_at(player.state().position);
let label = current.map(|chapter| chapter.title.as_str());
let samples = &watch.heatmap;
```

## 0. Video posters and avatar sources

The existing `images::image(url: &str, max_width: u32) -> Job<Arc<RgbaImage>>`
takes the drawn width in physical pixels. At 320 px and below, video posters use
`mqdefault`. From 321 through 480 px, try the 480 px HQ poster and then the 640 px SD
poster before HD. Crop the 4:3 sources to 16:9 and use Triangle for the small resize.
Wider requests try the renderer’s signed HD variant and the canonical HD poster first. The result can be smaller
than the request when YouTube only offers a smaller poster. Existing image decoding and caching
run on core workers. On the 125-poster, 384 px fixture replay, mean decode and resize fell
from 20.48 ms to 5.32 ms and the decoder threads used 0.66 CPU seconds, down from 2.32.
`NT_IMAGE_TRACE=1` reports per-image wall time and, on Linux, decoder-thread CPU time.
Image CDN requests keep their own concurrency but obey the shared cooldown: during one they
fail at once without a request, a CDN 429 or 403 starts it like any YouTube answer, and any
error other than a missing variant (404) ends the poster chain. Show the placeholder.

```rust
let width_px = (card_width * scale_factor).ceil() as u32;
let poster = nt_core::images::image(&video.thumbnail, width_px).await?;
draw_poster(&poster.pixels, poster.width, poster.height);
```

## 0b. Search notices and transcript fallback

```rust
pub enum SearchNotice {
    AgeGated { title: String, body: String },
    NoResults,
    Corrected { showing: String, original: String },
    DidYouMean { suggestion: String },
}
pub struct SearchPage {
    pub results: Page<Video>,
    pub notices: Vec<SearchNotice>,
    pub transcripts: Page<Video>,
    pub transcript_total: Option<u64>,
}
pub struct Moment {
    pub start_secs: f64,
    pub label: String,
    pub snippet: String,
    pub highlights: Vec<std::ops::Range<usize>>,
}
// New field on Video
pub moment: Option<Moment>,
impl Client {
    pub fn search_page(&self, query: &str) -> Job<SearchPage>;
}
impl Player {
    pub fn load_at(&self, video_id: &str, start_secs: f64) -> Result<()>;
}
```

Render native notices above the results. Use `Corrected::original` and
`DidYouMean::suggestion` as search links. If the transcript page has matches, label it
“Matches in video transcripts · search.pronsh.dev”. Render each snippet as plain text.
The highlight ranges are UTF-8 byte offsets into `Moment::snippet`.
Page either result section with `Client::more_videos` and that section's cursor.
The legacy `Client::search` returns the transcript page when that is the only source with
video matches. Use `search_page` to draw the notice and section label.

The fallback receives the query only after an empty YouTube video result. It has its own
request gate and a five-second limit. `NT_NO_TRANSCRIPT_SEARCH=1` disables it.
Opening a moment uses `load_at` from the event handler. A normal row has no moment.

Transcript rows arrive like YouTube rows: `Video::published` is already relative ("13 years
ago", "3 days ago", "today") and `Video::views` is short ("139K views"), so draw
`meta_line()` as for any card and drop any local ISO-date parsing. Core applies the rule
when parsing (no app called it directly, so it is not public): an ISO date-time reads like
`format::relative_time`; a date alone reads the same way in whole days, with "today" for
the current UTC day; any other text, such as YouTube's own "3 days ago" or "Streamed 2 years
ago", is kept unchanged.

```rust
let search = client().search_page(query).await?;
if let Some(video) = search.transcripts.items.first() {
    let at = video.moment.as_ref().map_or(0.0, |m| m.start_secs);
    player.load_at(&video.id, at)?;
}
```

## A3. Storyboards (seek-bar previews)

```rust
pub struct Storyboard { /* private */ }
pub struct StoryboardLevel { /* private */ }
impl Storyboard {
    pub fn levels(&self) -> &[StoryboardLevel];   // smallest first, never empty
}
impl StoryboardLevel {
    pub fn width(&self) -> u32; pub fn height(&self) -> u32;   // one frame, px
    pub fn count(&self) -> u32; pub fn columns(&self) -> u32; pub fn rows(&self) -> u32;
    pub fn interval(&self) -> f64;                             // seconds between frames
}
pub struct StoryboardFrame { pub sheet_url: String, pub x: u32, pub y: u32, pub width: u32, pub height: u32 }
impl Storyboard {
    pub fn level_for_width(&self, width_px: u32) -> &StoryboardLevel;
    pub fn preview(&self, seconds: f64, width_px: u32) -> Job<Arc<RgbaImage>>;
}
impl StoryboardLevel { pub fn frame_at(&self, seconds: f64) -> StoryboardFrame; }
// New fields
resolve::Streams::storyboard: Option<Storyboard>,
PlaybackState::storyboard: Option<Storyboard>,
```

The fields became private getters (no app read them): the parser rejects any level with a zero
size, a grid whose products overflow, or a sheet wider or taller than 16 384 px, so
`frame_at`, `level_for_width` and `preview` never panic, for any `seconds` (NaN and negative
read as 0).

The storyboard arrives with the `/player` answer, so it is in `player.state().storyboard` as
soon as the status leaves `Loading`; it costs no extra request until a preview is asked for.
While the pointer hovers or drags on the seek bar, call `preview(seconds_under_pointer,
drawn_width_px)` from the event handler and draw the returned frame (its own size, e.g. 160x90;
scale it to the tooltip) above the pointer with the time label. Keep showing the previous
preview until the new job finishes; drop results for a position the pointer has left.
`None` means the video has no storyboard: show only the time label.

A sheet holds 25 to 100 frames, so neighbouring positions come from the image cache (0.4 ms
measured) and only a new sheet costs a request (268 ms measured, through the polite gate,
newest request first). Sheets count against the shared image cache budget like thumbnails.

```rust
if let Some(sb) = &player.state().storyboard {
    let job = sb.preview(hover_secs, (160.0 * scale) as u32);
    // await, then draw_image(&frame.pixels, frame.width, frame.height)
}
```

## A4. Comment replies, sort orders, pinned and hearted

```rust
// New fields on Comment
pub replies: Option<Continuation>,   // None when the comment has no replies
pub pinned: bool,
pub hearted: bool,                   // the creator's heart
pub author_verified: bool,
// New field on CommentPage
pub sort: Vec<CommentSort>,          // first top-level page only
pub struct CommentSort { pub label: String, pub selected: bool, pub next: Continuation }
impl Client {
    pub fn more_replies(&self, next: &Continuation) -> Job<CommentPage>;
}
```

Draw "Pinned by <channel>" above a pinned comment, a small heart with the channel avatar for
`hearted`, a check after the author for `author_verified`. Under a comment with
`replies.is_some()` draw "▼ N replies" (`reply_count`); on click call
`more_replies(replies)` and list the answers indented under it, with "Show more replies"
while the reply page's `next` is `Some` (pass it to `more_replies` again). Reply pages have no
`sort` and no `total`. Draw a "Sort by" menu from `sort` (labels are YouTube's: "Top",
"Newest"); choosing one replaces the list with `more_comments(&sort.next)`. Keep the first
page's `sort` and `total` for the header; later pages don't repeat them.

```rust
let first = client().more_comments(watch.comments.as_ref().unwrap()).await?;
let newest = first.sort.iter().find(|s| s.label == "Newest").unwrap();
let page = client().more_comments(&newest.next).await?;
if let Some(r) = &page.items[0].replies { let replies = client().more_replies(r).await?; }
```

## A5. Related chips

```rust
pub struct Chip { pub label: String, pub selected: bool, pub next: Option<Continuation> }
// New field on WatchInfo
pub related_chips: Vec<Chip>,
```

Draw the chips above the related list when the list is not empty. The selected chip (`next:
None`) is the list already shown in `WatchInfo::related`; clicking another loads
`more_videos(chip.next)` and replaces the list (keep that page's `next` for scrolling).
Logged-out WEB answers currently carry no related chips (none of 54 recorded watch pages
does), so expect an empty list and draw nothing; the parser handles both shapes YouTube uses
(`chipCloudChipRenderer`, `chipViewModel`).

```rust
if let Some(chip) = watch.related_chips.get(i)
    && let Some(next) = chip.next.as_ref()
{
    related = client().more_videos(next).await?;
}
```

## A6. Badges, verified marks, upcoming

```rust
// New fields on Video
pub badges: Vec<String>,          // "4K", "CC", "New", "8K", "LIVE"
pub channel_verified: bool,
pub upcoming: Option<String>,     // "Premieres in 3 hours"
// New field on WatchInfo
pub channel_verified: bool,
// New field on ChannelInfo
pub verified: bool,
```

Draw `badges` as small grey labels in the card's metadata (YouTube shows them under the
views line on search rows and after it on grid cards), a check mark after the channel name
when `channel_verified`, and `upcoming` in place of the views line with no duration overlay.
Deviation: the brief asked for `ChannelRef.verified`, but apps/iced builds `ChannelRef`
literally, so a new field there would break it; the flag lives on `WatchInfo`, `ChannelInfo`
and `Video` instead.

```rust
draw_channel(&video.channel, video.channel_verified);
draw_badges(&video.badges);
if let Some(upcoming) = &video.upcoming { draw_schedule(upcoming); }
```

## A7. Audio tracks (dubs)

```rust
pub struct AudioTrack { pub id: String, pub label: String, pub original: bool, pub auto_dubbed: bool }
// New fields on PlaybackState
pub audio_tracks: Vec<AudioTrack>,  // original first; empty for a single language
pub audio_track: Option<String>,    // the id playing
impl Player {
    pub fn set_audio_track(&self, id: &str) -> Job<()>;
}
```

When `audio_tracks` is not empty, add "Audio track" to the settings menu with each `label`
(mark "auto-dubbed" tracks) and a check on `audio_track`. Call `set_audio_track(id)` from the
event handler; the video keeps playing, the new language opens on a core worker and replaces
the old one at the same position (measured 162 to 183 ms on 5C_HPTJg5ek, position kept). It
holds through a quality change or a link refresh and resets to the original on the next
`load`. Errors (not ready, unknown id, a stream that fails to open) come back in the job.
The new stream opens asynchronously inside mpv: the call returns at once and never blocks a
later `stop`, `load` or `set_quality` on the network (measured: `stop` 6 ms, `set_quality`
0.2 ms while a switch was opening). The choice is recorded before the stream opens, so a
quality change or link refresh started meanwhile reloads with the new language. A switch
superseded by a newer one, a `load` or a `stop` resolves to `Err(Cancelled)` (or `Ok` if a
reload already carries it).

```rust
let s = player.state();
if let Some(de) = s.audio_tracks.iter().find(|t| t.id.starts_with("de")) {
    player.set_audio_track(&de.id).await?;
}
```

## A8. Stable volume and loop

```rust
impl Player {
    pub fn set_normalize(&self, on: bool);   // YouTube gain; kept across videos
    pub fn set_loop(&self, on: bool);        // mpv loop-file; every load turns it off
}
// New fields on PlaybackState
pub normalize: bool,
pub looping: bool,
```

See C1 for the current implementation of Stable volume.

Show "Stable volume" as a toggle in the settings menu and "Loop" in the player's context menu
(and settings), both checked from the state. Both are immediate, no I/O; the state updates
and the waker runs. With loop on the video restarts at 0 instead of ending (no `Ended`
status, no autoplay). Normalization persists for the player's lifetime; if the app wants it
across launches, store the choice and call `set_normalize` after creating the player.

```rust
let state = player.state();
player.set_loop(!state.looping);
draw_loop_toggle(player.state().looping);
```

## B9. Search filters

```rust
pub enum SearchFilter { All, Params(String), Chip(Continuation), Transcripts }
pub struct SearchOption {
    pub label: String,
    pub selected: bool,
    pub filter: Option<SearchFilter>,
}
pub struct SearchFilterGroup { pub title: String, pub options: Vec<SearchOption> }
// New fields on SearchPage
pub chips: Vec<SearchOption>,
pub filters: Vec<SearchFilterGroup>,
impl Client {
    pub fn search_filtered(&self, query: &str, filter: &SearchFilter) -> Job<SearchPage>;
}
```

Start with `All` for the mixed results list. Draw the returned chips and group options in
order. A disabled option has no filter. Apply each click with the chosen filter and the same
query. The server returns updated choices when it includes the controls in its response.
A chip filter is tied to its original query. Keep the current controls if a subsequent page
omits them. `Transcripts` explicitly requests the transcript section.

```rust
let initial = client().search_filtered(query, &SearchFilter::All).await?;
if let Some(filter) = initial.chips.get(i).and_then(|o| o.filter.as_ref()) {
    let page = client().search_filtered(query, filter).await?;
}
```

## B10. Mixed search results

```rust
pub enum SearchResult { Video(Box<Video>), Playlist(PlaylistRef), Channel(SearchChannel), Shelf(SearchShelf) }
pub struct PlaylistRef {
    pub id: String, pub title: String, pub thumbnail: Option<String>,
    pub video_count: Option<String>,
}
pub struct SearchChannel { pub channel: ChannelRef, pub handle: Option<String>, pub verified: bool }
pub struct SearchShelf { pub title: String, pub items: Vec<SearchResult> }
// New field on SearchPage
pub entries: Vec<SearchResult>,
impl Client {
    pub fn more_search(&self, next: &Continuation) -> Job<SearchPage>;
}
```

Draw `entries` in order. Draw playlists as thumbnail stacks with their count, channels with
their avatar, handle and subscriber label, and shelf items under the shelf title. Page the
typed entries with `results.next` and `more_search`. The existing `results.items` continues
to expose a flat list of video rows.

```rust
let page = client().search_filtered(query, &SearchFilter::All).await?;
draw_entries(&page.entries);
if let Some(next) = &page.results.next {
    draw_entries(&client().more_search(next).await?.entries);
}
```

## C1. YouTube loudness gain

```rust
// New field on PlaybackState
pub normalization_gain_db: f64,
// PlaybackState only
pub normalization_supported: bool,
impl Player { pub fn set_normalize(&self, on: bool); }
```

Stable volume is on by default and applies YouTube’s attenuation for the selected audio
format independently of the volume slider. On libmpv before 0.38, gain is skipped.
Draw the setting where `normalization_supported` is true. Call the setter from an event
handler. The setting holds across video loads and switches to each dub’s own gain.

```rust
let state = player.state();
if state.normalization_supported {
    player.set_normalize(!state.normalize);
}
```

## C2. Prepare the next video

```rust
impl Player { pub fn prepare_next(&self, video_id: Option<&str>); }
```

Call from the event handler when the natural successor is known, or pass `None` when
autoplay is disabled or the queue ends. A different candidate cancels the old preparation.
A matching `load` or `load_at` reuses the resolved URLs at the same quality, or joins the
in-flight resolve. The operation returns immediately. It starts no media download while
preparing. Call again if the target size or quality changes. One live CLI run took 1045 ms
to the first frame with a fresh resolve and 475 ms with the successor prepared.

```rust
let next_id = autoplay.then_some(next_video.id.as_str());
player.prepare_next(next_id);
// Later, when the user clicks Next.
player.load(&next_video.id)?;
```

## C3. Continuation retries

The existing `Client` continuation methods retry transient failures twice, after 150 ms and
300 ms. Each attempt goes through the request gate. A refusal, fixture miss, invalid response,
or server-provided retry interval returns to the caller. Prefetch before scrolling reaches the
end. After exhaustion, show a Retry row in place and reuse the same cursor when activated.

```rust
let next = page.next.as_ref();
if let Some(cursor) = next {
    client().more_videos(cursor).on_done(send_continuation_result);
}
```

## C4. Core Clippy lints

The core manifest denies crash-prone production operations. The Clippy config allows
assertion helpers in unit tests. Integration test helpers outside a test function need
test-crate allowances.

## C5. Resume position

```rust
pub struct LastPlayed { pub id: String, pub position: f64, pub title: String }
impl Library { pub fn last_played(&self) -> Option<LastPlayed>; }
```

Read the latest checkpoint from a frontend event handler. Present it as a resume action and
pass its position to `load_at` when used. A main player records VOD playback about every ten
seconds while progressing, at stop, and on exit. Live playback is omitted.

```rust
impl Player { pub fn shutdown(&self); }   // stop, then write the library before returning
impl Library { pub fn flush(&self); }     // write now, blocking until written
```

At app exit call `player.shutdown()` and wait for it to return. It performs a synchronous
library write after stopping the player. For frontends with a playback command thread,
enqueue shutdown after any pending playback operations and wait in the close or quit hook.
iced and Rune keep their player in a static, so the exit hook is responsible for flushing
the final checkpoint. If no player has started, call `library().flush()`.

```rust
if let Some(resume) = library().last_played() {
    draw_resume_action(&resume.title);
    player.load_at(&resume.id, resume.position)?;
}
```

## C6. Wayland GPU video surface

```rust
// Linux only, re-exported from nt_core and nt_core::player.
pub struct VideoSurface { /* private */ }
impl VideoSurface {
    pub unsafe fn attach(
        player: &Player,
        display: raw_window_handle::RawDisplayHandle,
        window: raw_window_handle::RawWindowHandle,
        scale: f64,
    ) -> Result<Self>;
    pub fn set_rect(&self, x: i32, y: i32, width: u32, height: u32);
    pub fn set_clip(&self, clip: Option<(u32, u32, u32, u32)>);
    pub fn set_scale(&self, scale: f64);
    pub fn set_visible(&self, visible: bool);
    pub fn is_ready(&self) -> bool;
    pub fn is_presenting(&self) -> bool;
    pub fn error(&self) -> Option<String>;
    pub fn detach(self);
}
impl Player {
    pub fn presented_frames(&self) -> u64;
}
```

On the window-owning thread, request a transparent Wayland window with an alpha-capable swap
chain. Obtain the toolkit’s live Wayland display and window handles for that same window.
Use `attach` once the window exists, preferably before loading playback. It returns after
Wayland setup. The render worker initializes EGL and libmpv OpenGL. `is_ready()` reports
successful initialization independently of the first presentation. An EGL or GL failure
appears later in `error()` and falls back to software.

On Linux, each app calls `nt_core::gpu::select_egl_vendor()` before starting its runtime
or window toolkit. The startup function reads the Wayland compositor’s main DRM device
from linux-dmabuf feedback and selects the matching Mesa or NVIDIA GLVND manifest.
Explicit EGL or GLX vendor settings take precedence. Missing feedback, an unknown device,
an inherited Wayland socket, a process with multiple threads, or previously loaded EGL
leaves the loader configuration alone. No vendor selection is performed on other platforms.

```rust
// Call first in main, before starting any threads or initializing the window toolkit.
// SAFETY: no concurrent environment access is possible yet.
unsafe { nt_core::gpu::select_egl_vendor() };
```

Retain the resulting lease and detach it before
destroying the native window or display. X11 handles return an error. On macOS, continue
using the software player.

Set the rectangle relative to the **window**, in logical pixels, then redraw and commit
the parent. Pass the surface’s fractional scale and update it when the window changes outputs.
When the video is partially visible, pass the visible intersection to
`set_clip(Some((offset_x, offset_y, width, height)))`. The offsets are relative to the
destination's upper-left corner, in logical pixels. Clipping preserves the picture's scale.
Use `set_clip(None)` for the full rectangle. If the intersection is empty or the watch
surface is hidden, call `set_visible(false)` and stop cutting the hole.

Paint opaque UI around the visible rectangle. Cut the hole when the lease is presenting
and the current load has `player.presented_frames() > 0`. The GPU count resets on load
and stop and counts only swaps that carry a newly decoded picture of the current file (mpv's
next-frame info: present and not a redraw), so the initial black render used to map the
child never opens the hole, and a picture of the previous file that was mid-render during
a load is never counted. The first picture counts as soon as it is swapped, without waiting
for mpv's playback-restart event (that wait cost one frame interval, 33 ms at 30 fps), and the 0 → 1 step wakes the app through the player's
waker, so a frontend that cuts the hole from its wake handler needs no polling. Keeping the
child visible (`set_visible(true)`) while the next video loads lets the worker present the
first decoded frame without waiting for the UI to see `Ready`; the parent's poster still
covers it until the count rises. A clip with no area unmaps the child (same as hidden) and a
later non-empty clip maps it again. Leave the hole transparent through compositing
and omit it from any parent opaque region. Draw controls over it. The child has an empty
input region.

Use `Player::frame` if attach returns an error, after detaching, or if the lease reports an
error. The worker reinstates the software renderer and reopens the current stream around
the saved position with its already-resolved URLs. A normal Watch load uses the same
metadata requests in either mode. The GPU path wakes the app for presentation and playback
state changes, while EGL swaps each picture independently.

```rust
let hardware = unsafe { VideoSurface::attach(&player, display, window, scale) }.ok();
if let Some(surface) = hardware.as_ref() {
    surface.set_rect(x, y, width, height);
    if surface.is_presenting() && player.presented_frames() > 0 {
        cut_video_hole();
    } else {
        paint_poster();
    }
}
```

With `NT_GPU_TRACE=1`, core logs the elapsed worker initialization time. A paired
release-app run on Intel Arrow Lake used the same local clip and replayed feed, then paused
with the surface attached. Default GLVND discovery and normal startup respectively took
171.2 and 105.8 ms for iced, 27.1 and 25.4 ms for GPUI, and 505.3 and 99.0 ms for Rune.
The normal startup processes all mapped Mesa EGL and omitted NVIDIA EGL. These are
single-run samples and include any driver work still outstanding when the worker starts.

The `wayland_video` example measured renderer initialization independently of the
attach call and the first playback frame on Intel Arrow Lake with a second NVIDIA GPU.
Standard GLVND discovery took 153.3 ms and added 39.3 MiB PSS at renderer initialization.
Selecting Mesa reduced this to 63.9 ms and 20.1 MiB. The lease returned in under 1 ms
in both cases. Six local warm switches reached Ready in 8.7–11.6 ms and first presentation
in 26.8–34.8 ms with Mesa selected. The render context is reused across loads. Network
loads consume the same matching `prepare_next` result in either render mode. The local
switch fixture measures decode and presentation, with no network or prefetch involved.

## B11. Channel tabs and About

```rust
pub enum ChannelTab { Home, Videos, Live, Playlists }
pub struct ChannelLink { pub label: String, pub url: String }
pub struct ChannelAbout { pub description: String, pub links: Vec<ChannelLink> }
// New fields on ChannelInfo
pub tabs: Vec<ChannelTab>,
pub selected_tab: Option<ChannelTab>,
pub video_sort: Vec<Chip>,
pub playlists: Page<PlaylistRef>,
pub featured: Option<Video>,
pub links_label: Option<String>,
pub links: Vec<ChannelLink>,
pub about: Option<Continuation>,
impl Client {
    pub fn channel_tab(&self, channel_id: &str, tab: ChannelTab) -> Job<ChannelInfo>;
    pub fn more_playlists(&self, next: &Continuation) -> Job<Page<PlaylistRef>>;
    pub fn channel_about(&self, next: &Continuation) -> Job<ChannelAbout>;
}
```

Draw available tabs in order. Use the Videos sort cursors with `more_videos` and replace
the current page on a sort change. In Playlists, draw the typed playlist cards. On Home,
show the featured video if supplied. Draw the links summary and let the user expand About
to see the full description and link collection.

```rust
let channel = client().channel_tab(id, ChannelTab::Videos).await?;
if let Some(about) = &channel.about {
    let expanded = client().channel_about(about).await?;
    draw_about(&expanded.description, &expanded.links);
}
```

## B12. Playlist and mix panels

```rust
pub struct PlaylistPanel {
    pub id: String, pub title: String, pub items: Vec<Video>,
    pub index: Option<usize>, pub next: Option<Continuation>, pub is_mix: bool,
}
impl PlaylistPanel {
    pub fn next_video(&self) -> Option<&Video>;
    pub fn previous_video(&self) -> Option<&Video>;
}
// New field on WatchInfo
pub playlist: Option<PlaylistPanel>,
impl Client {
    pub fn watch_in_playlist(&self, video_id: &str, playlist_id: &str) -> Job<WatchInfo>;
    pub fn playlist(&self, playlist_id: &str) -> Job<PlaylistPanel>;
    pub fn more_playlist_videos(&self, next: &Continuation) -> Job<Page<Video>>;
}
```

Use `playlist` to open a playlist card and `watch_in_playlist` when playing a row from it.
The Watch request returns the current panel in the existing `/next` response. Highlight
its zero-based index. Use the adjacent item helpers for navigation. Fetch with the panel’s
cursor before reaching the end and append its returned items. Use a mix’s Watch video
context when the mix ID alone does not identify a starting video.

```rust
let info = client().watch_in_playlist(video_id, playlist_id).await?;
if let Some(panel) = info.playlist.as_ref() {
    draw_playlist(&panel.title, &panel.items, panel.index);
    player.prepare_next(panel.next_video().map(|v| v.id.as_str()));
}
```

## B13. Local queue and hide lists

```rust
impl Library {
    pub fn queue(&self) -> Vec<Video>;
    pub fn enqueue(&self, video: &Video);
    pub fn remove_queued(&self, video_id: &str);
    pub fn pop_queued(&self) -> Option<Video>;
    pub fn set_video_hidden(&self, video_id: &str, hidden: bool);
    pub fn set_channel_hidden(&self, channel_id: &str, hidden: bool);
    pub fn is_hidden(&self, video: &Video) -> bool;
    pub fn flush(&self);
}
```

Queue order is insertion order. An existing video ID stays in place when added again.
Show the queue from a snapshot and consume its head only when advancing. On “Not interested”
or “Don’t recommend channel,” set the corresponding flag and use `is_hidden` to update
the visible list immediately. Fresh Home, Search, and related pages and continuations apply
the saved preferences. With a nonempty continuation, an empty filtered page can still have
more results. All updates are local and saved atomically on a worker. Short-lived CLI
commands can call the synchronous `flush` method before exit.

```rust
library().enqueue(&video);
let queue = library().queue();
player.prepare_next(queue.first().map(|video| video.id.as_str()));
```

## B14. Home feed chips

```rust
pub struct HomeFeed { pub videos: Page<Video>, pub chips: Vec<Chip> }
impl Client { pub fn home_feed(&self) -> Job<HomeFeed>; }
```

Draw returned chips in order and apply their cursor using `more_videos`. When the chip list
is empty, draw `TOPICS` and call `home(topic.id)` for each. NativeTube is always logged out,
and YouTube's logged-out Home (`FEwhat_to_watch`) is always empty, so `home_feed` no longer
asks for it: it returns `home("all")` (5-minute disk cache included) with no chips, at once
from the cache on a warm start. Nothing changes for apps.

```rust
let feed = client().home_feed().await?;
if feed.chips.is_empty() { draw_topics(TOPICS); }
else { draw_chips(&feed.chips); }
draw_videos(&feed.videos.items);
```

## B15. Muted hover preview

```rust
impl Player {
    pub fn new_preview(format: PixelFormat, waker: impl Fn() + Send + Sync + 'static)
        -> Result<Player>;
}
```

Create one reusable preview per app when needed. It is muted, uses no audio stream or
MPRIS, does not write resume state, and resolves only video at 360p or lower. Stop it when
the hover ends or a row leaves the viewport. Draw the poster until `frame()` arrives,
or keep the poster if a low-resolution stream is unavailable.

```rust
let preview = Player::new_preview(PixelFormat::Rgba, wake_preview)?;
preview.set_target_size(320, 180);
preview.load(&video.id)?;
if let Some(frame) = preview.frame() { draw_frame(&frame); }
```

On `5C_HPTJg5ek`, `ntctl preview` rendered 640×360 in 612 ms to the first frame.
The process used 0.65 CPU seconds during the five seconds after startup plus time to the
first frame. Resident memory increased by 38.2 MiB relative to the idle preview instance.
These are single-run measurements on the test laptop.


## C7. Caption position

```rust
impl Player {
    pub fn set_caption_position(&self, percent: f64);
}
```

mpv draws captions into the video picture on both the GPU surface and the software path, so a
frontend can't move them with its own layout. This sets mpv's `sub-pos`: the caption block's
vertical position in percent of the picture's height (100 = the normal bottom position, smaller
= higher). It is immediate, does no I/O, is kept across loads, and clamps to 0–150; non-finite
values are ignored.

Use it the way YouTube places captions: while the control bar shows, lift the captions above the
progress and control bars; when the controls hide, ease back to about 8% from the bottom
(≈200 ms). Recompute on caption toggle, video ready, theatre, fullscreen and resize; letterbox
bars under the picture reduce how far the captions must rise. Send a new value only when it
changes. Reference implementation: apps/iced/src/app.rs (added 2026-11-19).
