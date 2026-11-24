//! The YouTube client every frontend calls. Work runs on the core's own tokio
//! runtime; each call returns a [`Job`], a runtime-agnostic future, so iced,
//! GPUI and Rune can each await it (or poll it, or take a callback) on their
//! own executors.

use crate::error::{Error, Result};
use crate::fixtures::Fixtures;
use crate::innertube::{self, SEARCH_VIDEOS_ONLY};
use crate::model::*;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use tokio::sync::oneshot;

/// Search filter: uploaded this week, videos only. Used for home topics so the feed is fresh.
const THIS_WEEK_VIDEOS: &str = "EgQIAxAB";
/// The topics interleaved into the "All" home feed.
const ALL_MIX: &[&str] = &["music", "gaming", "tech", "science", "news", "sports"];

/// A result that arrives later. Await it, block on it, poll it, or hand it a callback.
#[must_use = "a Job does nothing visible unless awaited, polled, or given a callback"]
pub struct Job<T> {
    rx: oneshot::Receiver<Result<T>>,
}

impl<T: Send + 'static> Job<T> {
    pub(crate) fn spawn<F>(fut: F) -> Job<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let rt = match try_runtime() {
            Ok(rt) => rt,
            Err(e) => return Job::ready(Err(e)),
        };
        let (tx, rx) = oneshot::channel();
        rt.spawn(async move {
            let _ = tx.send(fut.await);
        });
        Job { rx }
    }

    pub(crate) fn from_receiver(rx: oneshot::Receiver<Result<T>>) -> Job<T> {
        Job { rx }
    }

    /// A job that has already finished.
    pub(crate) fn ready(value: Result<T>) -> Job<T> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(value);
        Job { rx }
    }

    /// Block the calling thread until the result arrives. Never call this on a UI thread.
    pub fn wait(self) -> Result<T> {
        self.rx.blocking_recv().unwrap_or(Err(Error::Cancelled))
    }

    /// Take the result if it has arrived. `None` means "not yet".
    pub fn try_take(&mut self) -> Option<Result<T>> {
        match self.rx.try_recv() {
            Ok(v) => Some(v),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(Error::Cancelled)),
        }
    }

    /// Run `f` with the result on a core worker thread once it arrives.
    pub fn on_done(self, f: impl FnOnce(Result<T>) + Send + 'static) {
        match try_runtime() {
            Ok(rt) => drop(rt.spawn(async move { f(self.await) })),
            Err(e) => f(Err(e)),
        }
    }
}

impl<T> Future for Job<T> {
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|r| r.unwrap_or(Err(Error::Cancelled)))
    }
}

/// The core's runtime: two worker threads plus a blocking pool for decoding. Starting it can
/// fail (no threads or no epoll left); then every core job resolves to that error.
pub fn try_runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_name("nt-core")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| Error::Io(format!("could not start the nt-core runtime: {e}")))
}

struct RequestFailure {
    error: Error,
    retry: bool,
}

impl From<Error> for RequestFailure {
    fn from(error: Error) -> Self {
        Self {
            error,
            retry: false,
        }
    }
}

impl From<reqwest::Error> for RequestFailure {
    fn from(error: reqwest::Error) -> Self {
        let mut source: &dyn std::error::Error = &error;
        let mut retry = error.is_timeout();
        while !retry {
            if let Some(io) = source.downcast_ref::<std::io::Error>() {
                retry = matches!(
                    io.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                );
                if retry {
                    break;
                }
            }
            let Some(next) = source.source() else {
                break;
            };
            source = next;
        }
        Self {
            error: error.into(),
            retry,
        }
    }
}

async fn retry_page<T, F>(continuation: bool, mut request: impl FnMut() -> F) -> Result<T>
where
    F: Future<Output = std::result::Result<T, RequestFailure>> + Send,
{
    let delays = [150, 300];
    let mut delays = delays.into_iter().filter(|_| continuation);
    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let Some(delay) = delays.next().filter(|_| error.retry) else {
                    return Err(error.error);
                };
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    }
}

#[derive(Clone)]
pub struct Client {
    http: Result<reqwest::Client>,
    fixtures: Option<Fixtures>,
}

/// The process-wide client. Reads `NT_FIXTURES` / `NT_FIXTURE_MODE` once.
pub fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::from_env)
}

impl Client {
    pub fn from_env() -> Client {
        let http = reqwest::Client::builder()
            .user_agent(innertube::USER_AGENT)
            .pool_max_idle_per_host(8)
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(Error::from);
        Client {
            http,
            fixtures: Fixtures::from_env(),
        }
    }

    fn http(&self) -> Result<&reqwest::Client> {
        self.http.as_ref().map_err(Clone::clone)
    }

    pub(crate) async fn get_bytes(&self, url: &str) -> Result<bytes::Bytes> {
        self.get_bytes_if_present(url)
            .await?
            .ok_or_else(|| Error::Network("HTTP 404 (image unavailable)".into()))
    }

    pub(crate) async fn get_bytes_if_present(&self, url: &str) -> Result<Option<bytes::Bytes>> {
        let key = format!("GET {url}");
        if let Some(f) = &self.fixtures
            && let Some(hit) = f.lookup(&key)?
        {
            return Ok(Some(hit));
        }
        // Static image CDNs aren't YouTube's API; everything else waits its turn at the gate.
        let is_cdn = reqwest::Url::parse(url).ok().is_some_and(|url| {
            let host = url.host_str().unwrap_or_default();
            ["ytimg.com", "ggpht.com", "googleusercontent.com"]
                .iter()
                .any(|suffix| {
                    host == *suffix
                        || host
                            .strip_suffix(suffix)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                })
                && !url.path().starts_with("/sb/")
        });
        // The image CDN keeps its own concurrency (images.rs) but obeys the shared cooldown.
        let _turn = if is_cdn {
            crate::polite::cooldown()?;
            None
        } else {
            Some(crate::polite::turn().await?)
        };
        let resp = self.http()?.get(url).send().await?;
        if is_cdn {
            if !cdn_status(
                resp.status(),
                resp.headers().get(reqwest::header::RETRY_AFTER),
            )? {
                return Ok(None);
            }
        } else {
            crate::polite::check_status(
                resp.status(),
                resp.headers().get(reqwest::header::RETRY_AFTER),
            )?;
        }
        let body = resp.bytes().await?;
        if let Some(f) = &self.fixtures {
            f.store(&key, &body);
        }
        Ok(Some(body))
    }

    async fn post(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        // The key leaves out the context so a client-version bump doesn't invalidate recordings.
        let key = format!("POST {endpoint} {}", canonical_json(&body));
        if let Some(f) = &self.fixtures
            && let Some(hit) = f.lookup(&key)?
        {
            return Ok(serde_json::from_slice(&hit)?);
        }
        body["context"] = innertube::context();
        let url = format!("https://www.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false");
        let raw = retry_page(body.get("continuation").is_some(), || async {
            let _turn = crate::polite::turn().await?;
            let resp = self
                .http()?
                .post(&url)
                .header("Origin", "https://www.youtube.com")
                .header("X-Youtube-Client-Name", "1")
                .header("X-Youtube-Client-Version", innertube::CLIENT_VERSION)
                .json(&body)
                .send()
                .await?;
            let status = resp.status();
            let retry_after = resp.headers().get(reqwest::header::RETRY_AFTER);
            crate::polite::check_status(status, retry_after).map_err(|error| RequestFailure {
                error,
                retry: status.is_server_error() && retry_after.is_none(),
            })?;
            Ok(resp.bytes().await?)
        })
        .await?;
        if let Some(f) = &self.fixtures {
            f.store(&key, &raw);
        }
        Ok(serde_json::from_slice(&raw)?)
    }

    async fn search_raw(&self, query: String, params: &'static str) -> Result<Page<Video>> {
        let params = params.replace("%3D", "=");
        let v = self
            .post("search", json!({ "query": query, "params": params }))
            .await?;
        innertube::parse_search(&v)
    }

    async fn transcript_page(&self, query: &str, page: u32) -> Result<(Page<Video>, u64)> {
        if std::env::var("NT_NO_TRANSCRIPT_SEARCH").as_deref() == Ok("1") {
            return Ok((Page::default(), 0));
        }
        let url = format!(
            "https://search.pronsh.dev/api/search?q={}&page={page}&per_page=20",
            urlencode(query)
        );
        let key = format!("GET {url}");
        if let Some(f) = &self.fixtures
            && let Some(hit) = f.lookup(&key)?
        {
            return crate::transcripts::parse(&hit, query, page);
        }
        let request = retry_page(page > 1, || async {
            let _turn = crate::polite::transcript_turn().await?;
            let resp = self.http()?.get(&url).send().await?;
            let status = resp.status();
            if !status.is_success() {
                return Err(RequestFailure {
                    error: Error::Network(format!("Transcript search HTTP {status}")),
                    retry: status.is_server_error()
                        && !resp.headers().contains_key(reqwest::header::RETRY_AFTER),
                });
            }
            Ok(resp.bytes().await?)
        });
        let body = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .map_err(|_| Error::Network("Transcript search timed out".into()))??;
        if let Some(f) = &self.fixtures {
            f.store(&key, &body);
        }
        crate::transcripts::parse(&body, query, page)
    }

    async fn more_raw(&self, c: Continuation) -> Result<Page<Video>> {
        if c.endpoint == "transcript-search" {
            let (query, index): (String, u32) = serde_json::from_str(&c.token)?;
            return Ok(self.transcript_page(&query, index).await?.0);
        }
        if c.endpoint == "multi" {
            return self.more_multi(c.token).await;
        }
        let v = self
            .post(&c.endpoint, json!({ "continuation": c.token }))
            .await?;
        let mut page = innertube::parse_video_continuation(&v, &c.endpoint)?;
        if let Some((id, name, avatar)) = &c.owner {
            for video in &mut page.items {
                video.channel = name.clone();
                video.channel_id = Some(id.clone());
                video.channel_avatar = avatar.clone();
            }
            if let Some(next) = &mut page.next {
                next.owner = c.owner.clone();
            }
        }
        Ok(page)
    }

    /// Fetch several pages at once and deal their items out in turn, so the "All" feed mixes topics.
    async fn more_multi(&self, tokens: String) -> Result<Page<Video>> {
        let tokens: Vec<String> = serde_json::from_str(&tokens)?;
        let mut jobs = Vec::new();
        for token in tokens {
            let this = self.clone();
            jobs.push(tokio::spawn(async move {
                let v = this
                    .post("search", json!({ "continuation": token }))
                    .await?;
                innertube::parse_video_continuation(&v, "search")
            }));
        }
        let mut pages = Vec::new();
        for job in jobs {
            match job.await {
                Ok(Ok(page)) => pages.push(page),
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(Error::Cancelled),
            }
        }
        Ok(interleave(pages))
    }

    async fn home_raw(&self, topic: String) -> Result<Page<Video>> {
        let topic = TOPICS.iter().find(|t| t.id == topic).unwrap_or(&TOPICS[0]);
        if !topic.query.is_empty() {
            return self
                .search_raw(topic.query.to_string(), THIS_WEEK_VIDEOS)
                .await;
        }
        let mut jobs = Vec::new();
        for id in ALL_MIX {
            let query = TOPICS
                .iter()
                .find(|t| t.id == *id)
                .map(|t| t.query)
                .unwrap_or(id);
            let this = self.clone();
            jobs.push(tokio::spawn(async move {
                this.search_raw(query.to_string(), THIS_WEEK_VIDEOS).await
            }));
        }
        let mut pages = Vec::new();
        for job in jobs {
            // One topic failing shouldn't blank the whole home feed.
            if let Ok(Ok(page)) = job.await {
                pages.push(page);
            }
        }
        if pages.is_empty() {
            return Err(Error::Network("every home topic failed to load".into()));
        }
        Ok(interleave(pages))
    }

    // ---- public API -------------------------------------------------------------------------

    /// The home feed for a topic id from [`TOPICS`] (unknown ids fall back to "all").
    ///
    /// The last answer per topic is kept on disk for [`HOME_CACHE_SECS`], so a relaunch shows the
    /// feed at once and switching back to a topic doesn't ask YouTube again. Recorded test data
    /// (`NT_FIXTURES`) bypasses this cache, so benchmarks stay deterministic.
    pub fn home(&self, topic: &str) -> Job<Page<Video>> {
        let (this, topic) = (self.clone(), topic.to_string());
        Job::spawn(async move {
            let cached = this.fixtures.is_none().then(|| home_cache_path(&topic));
            if let Some(path) = &cached
                && let Some(page) = read_fresh(path, HOME_CACHE_SECS)
            {
                return Ok(visible_videos(page));
            }
            let page = this.home_raw(topic).await?;
            if let Some(path) = cached
                && let Ok(json) = serde_json::to_vec(&page)
            {
                // A ~100 KB file, written before returning so a short-lived caller keeps it too.
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
            Ok(visible_videos(page))
        })
    }

    /// The Home feed. NativeTube is always logged out, and YouTube's logged-out Home
    /// (`FEwhat_to_watch`) is empty, so this is the "all" topic mix of [`Self::home`] (with its
    /// 5-minute cache) and no chips: draw [`TOPICS`] as the chips. No request is spent on the
    /// empty feed.
    pub fn home_feed(&self) -> Job<HomeFeed> {
        let all = self.home("all");
        Job::spawn(async move {
            Ok(HomeFeed {
                videos: all.await?,
                chips: Vec::new(),
            })
        })
    }

    /// Video search results. When YouTube has no matches, returns transcript matches instead.
    pub fn search(&self, query: &str) -> Job<Page<Video>> {
        let search = self.search_filtered(query, &SearchFilter::Params(SEARCH_VIDEOS_ONLY.into()));
        Job::spawn(async move {
            let page = search.await?;
            if page.results.items.is_empty() && !page.transcripts.items.is_empty() {
                Ok(page.transcripts)
            } else {
                Ok(page.results)
            }
        })
    }

    /// Video results, notices from YouTube, and transcript fallback as a separate section.
    pub fn search_page(&self, query: &str) -> Job<SearchPage> {
        self.search_filtered(query, &SearchFilter::Params(SEARCH_VIDEOS_ONLY.into()))
    }

    /// Search using an option from the current chips or filters, or start with All.
    pub fn search_filtered(&self, query: &str, filter: &SearchFilter) -> Job<SearchPage> {
        let (this, query, filter) = (self.clone(), query.trim().to_string(), filter.clone());
        Job::spawn(async move {
            if filter == SearchFilter::Transcripts {
                let (transcripts, total) = this.transcript_page(&query, 1).await?;
                let mut page = SearchPage {
                    transcripts,
                    transcript_total: Some(total),
                    ..Default::default()
                };
                crate::library().filter_search(&mut page);
                return Ok(page);
            }
            let (body, continuation) = match &filter {
                SearchFilter::All => (json!({ "query": query }), false),
                SearchFilter::Params(params) => (
                    json!({ "query": query, "params": params.replace("%3D", "=") }),
                    false,
                ),
                SearchFilter::Chip(c) if c.endpoint == "search" => {
                    (json!({ "continuation": c.token }), true)
                }
                _ => return Err(Error::Parse("Expected a search filter".into())),
            };
            let v = this.post("search", body).await?;
            let mut page = innertube::search_page(&v, &query, continuation)?;
            let basic = matches!(filter, SearchFilter::All)
                || matches!(&filter, SearchFilter::Params(s) if s.replace("%3D", "=") == "EgIQAQ==");
            if basic
                && page.results.items.is_empty()
                && !query.is_empty()
                && std::env::var("NT_NO_TRANSCRIPT_SEARCH").as_deref() != Ok("1")
                && let Ok((transcripts, total)) = this.transcript_page(&query, 1).await
            {
                page.transcripts = transcripts;
                page.transcript_total = Some(total);
            }
            crate::library().filter_search(&mut page);
            Ok(page)
        })
    }

    /// Continue the typed search list, preserving non-video result types.
    pub fn more_search(&self, next: &Continuation) -> Job<SearchPage> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            if next.endpoint != "search" {
                return Err(Error::Parse("Expected a search continuation".into()));
            }
            let response = this
                .post("search", json!({ "continuation": next.token }))
                .await?;
            let mut page = innertube::search_page(&response, "", true)?;
            crate::library().filter_search(&mut page);
            Ok(page)
        })
    }

    /// The next page of any video list (home, search, related, channel).
    pub fn more_videos(&self, next: &Continuation) -> Job<Page<Video>> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let filter = next.owner.is_none();
            let page = this.more_raw(next).await?;
            Ok(if filter { visible_videos(page) } else { page })
        })
    }

    /// Title, channel, description, likes, related videos and the comments cursor.
    pub fn watch(&self, video_id: &str) -> Job<WatchInfo> {
        let (this, id) = (self.clone(), video_id.to_string());
        Job::spawn(async move {
            let v = this.post("next", json!({ "videoId": id })).await?;
            let mut info = innertube::parse_watch(&v, &id)?;
            info.related = visible_videos(info.related);
            Ok(info)
        })
    }

    /// Open Watch in the context of a YouTube playlist or mix.
    pub fn watch_in_playlist(&self, video_id: &str, playlist_id: &str) -> Job<WatchInfo> {
        let (this, id, playlist) = (self.clone(), video_id.to_owned(), playlist_id.to_owned());
        Job::spawn(async move {
            let v = this
                .post("next", json!({ "videoId": id, "playlistId": playlist }))
                .await?;
            let mut info = innertube::parse_watch(&v, &id)?;
            info.related = visible_videos(info.related);
            info.playlist = innertube::playlist_panel(&v, Some(&playlist), Some(&id));
            Ok(info)
        })
    }

    /// Fetch playlist videos and the initial selection. Mixes are opened through Watch.
    pub fn playlist(&self, playlist_id: &str) -> Job<PlaylistPanel> {
        let (this, id) = (self.clone(), playlist_id.to_owned());
        Job::spawn(async move {
            if id.starts_with("RD") {
                let mut body = json!({ "playlistId": id });
                if let Some(seed) = id.strip_prefix("RD").filter(|seed| seed.len() == 11) {
                    body["videoId"] = json!(seed);
                }
                let v = this.post("next", body).await?;
                return innertube::playlist_panel(&v, Some(&id), None)
                    .ok_or_else(|| Error::Parse("Mix has no playlist panel".into()));
            }
            let v = this
                .post("browse", json!({ "browseId": format!("VL{id}") }))
                .await?;
            let page = innertube::playlist_items(&v);
            let title = innertube::text(&v["metadata"]["playlistMetadataRenderer"]["title"])
                .ok_or_else(|| Error::Parse("Playlist has no title".into()))?;
            Ok(PlaylistPanel {
                id,
                title,
                index: (!page.items.is_empty()).then_some(0),
                items: page.items,
                next: page.next,
                is_mix: false,
            })
        })
    }

    /// Fetch the next batch from a playlist or watch panel.
    pub fn more_playlist_videos(&self, next: &Continuation) -> Job<Page<Video>> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let v = this
                .post(&next.endpoint, json!({ "continuation": next.token }))
                .await?;
            let mut page = innertube::playlist_items(&v);
            if let Some(cursor) = &mut page.next {
                cursor.endpoint = next.endpoint;
            }
            Ok(page)
        })
    }

    /// A page of top comments. Pass [`WatchInfo::comments`] first, then each page's `next`, or a
    /// [`CommentSort::next`] from the first page to reload in that order.
    pub fn more_comments(&self, next: &Continuation) -> Job<CommentPage> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let v = this
                .post("next", json!({ "continuation": next.token }))
                .await?;
            innertube::parse_comments(&v)
        })
    }

    /// A page of replies to one comment. Pass [`Comment::replies`] first, then each page's `next`.
    pub fn more_replies(&self, next: &Continuation) -> Job<CommentPage> {
        self.more_comments(next)
    }

    /// A channel's header and the first page of its uploads.
    pub fn channel(&self, channel_id: &str) -> Job<ChannelInfo> {
        self.channel_tab(channel_id, ChannelTab::Videos)
    }

    /// Browse one of the tabs advertised by a channel.
    pub fn channel_tab(&self, channel_id: &str, tab: ChannelTab) -> Job<ChannelInfo> {
        let (this, id) = (self.clone(), channel_id.to_string());
        Job::spawn(async move {
            let params = match tab {
                ChannelTab::Home => "EghmZWF0dXJlZPIGBAoCMgA%3D",
                ChannelTab::Videos => innertube::CHANNEL_VIDEOS_TAB,
                ChannelTab::Live => "EgdzdHJlYW1z8gYECgJ6AA%3D%3D",
                ChannelTab::Playlists => "EglwbGF5bGlzdHPyBgoKCEIGCgIQaCIA",
            };
            let v = this
                .post("browse", json!({ "browseId": id, "params": params }))
                .await?;
            innertube::parse_channel(&v, &id)
        })
    }

    /// Continue a page from the Playlists tab.
    pub fn more_playlists(&self, next: &Continuation) -> Job<Page<PlaylistRef>> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let v = this
                .post("browse", json!({ "continuation": next.token }))
                .await?;
            Ok(innertube::playlists(&v))
        })
    }

    /// Fetch the expanded channel About panel when the header provides a cursor.
    pub fn channel_about(&self, next: &Continuation) -> Job<ChannelAbout> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let v = this
                .post("browse", json!({ "continuation": next.token }))
                .await?;
            innertube::channel_about(&v)
        })
    }

    /// Search-as-you-type suggestions. Cheap; call it on each keystroke (debounced).
    pub fn suggestions(&self, prefix: &str) -> Job<Vec<String>> {
        let (this, prefix) = (self.clone(), prefix.trim().to_string());
        Job::spawn(async move {
            if prefix.is_empty() {
                return Ok(Vec::new());
            }
            let url = format!(
                "https://suggestqueries-clients6.youtube.com/complete/search?client=youtube&ds=yt&hl=en&q={}",
                urlencode(&prefix)
            );
            let body = this.get_bytes(&url).await?;
            Ok(innertube::parse_suggestions(&String::from_utf8_lossy(
                &body,
            )))
        })
    }

    /// Newest uploads across the subscribed channels, newest first, from their public feeds.
    pub fn subscription_feed(&self, channels: Vec<ChannelRef>) -> Job<Vec<Video>> {
        let this = self.clone();
        Job::spawn(async move {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let mut jobs = Vec::new();
            for ch in channels.into_iter().take(40) {
                let this = this.clone();
                jobs.push(tokio::spawn(async move {
                    let url = format!(
                        "https://www.youtube.com/feeds/videos.xml?channel_id={}",
                        ch.id
                    );
                    let xml = this.get_bytes(&url).await?;
                    let mut items = innertube::parse_feed(&String::from_utf8_lossy(&xml), now)?;
                    for (_, v) in &mut items {
                        v.channel_avatar = ch.avatar.clone();
                    }
                    Ok::<_, Error>(items)
                }));
            }
            let mut all = Vec::new();
            for job in jobs {
                if let Ok(Ok(items)) = job.await {
                    all.extend(items);
                }
            }
            all.sort_by_key(|(published, _)| std::cmp::Reverse(*published));
            Ok(all.into_iter().map(|(_, v)| v).take(120).collect())
        })
    }
}

fn visible_videos(mut page: Page<Video>) -> Page<Video> {
    crate::library().filter_videos(&mut page.items);
    page
}

/// An image CDN answer: `Ok(true)` to read the body, `Ok(false)` for a missing variant (the
/// poster chain tries the next one). 429 and 403 start the shared cooldown like any YouTube
/// answer; those and other failures end the chain with an error.
fn cdn_status(
    status: reqwest::StatusCode,
    retry_after: Option<&reqwest::header::HeaderValue>,
) -> Result<bool> {
    if matches!(
        status,
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
    ) {
        return Ok(false);
    }
    crate::polite::check_status(status, retry_after)?;
    Ok(true)
}

/// How long a home feed is reused from disk.
pub const HOME_CACHE_SECS: u64 = 5 * 60;

fn home_cache_path(topic: &str) -> std::path::PathBuf {
    let safe: String = topic
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    crate::library::data_dir().join("cache").join(format!(
        "home-{}.json",
        if safe.is_empty() { "all" } else { &safe }
    ))
}

/// A cached page if the file is younger than `max_age_secs` and still parses.
fn read_fresh(path: &std::path::Path, max_age_secs: u64) -> Option<Page<Video>> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    if age.as_secs() >= max_age_secs {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Deal items from several pages out in turn, skipping duplicates. Continuations become one "multi" cursor.
fn interleave(pages: Vec<Page<Video>>) -> Page<Video> {
    let longest = pages.iter().map(|p| p.items.len()).max().unwrap_or(0);
    let mut items: Vec<Video> = Vec::new();
    for i in 0..longest {
        for page in &pages {
            if let Some(v) = page.items.get(i)
                && !items.iter().any(|x| x.id == v.id)
            {
                items.push(v.clone());
            }
        }
    }
    let tokens: Vec<&str> = pages
        .iter()
        .filter_map(|p| p.next.as_ref())
        .map(|c| c.token.as_str())
        .collect();
    let next = (!tokens.is_empty()).then(|| Continuation {
        endpoint: "multi".into(),
        token: serde_json::to_string(&tokens).unwrap_or_default(),
        owner: None,
    });
    Page { items, next }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// JSON text with object keys sorted, so it is the same whether or not some dependency turned on
/// serde_json's `preserve_order` feature (GPUI's tree does; a fixture key must not depend on it).
pub(crate) fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs in a child process: a cooldown is process-wide and would stall the other tests.
    #[test]
    fn image_cdn_respects_the_cooldown() {
        const CHILD: &str = "NT_TEST_CDN_COOLDOWN";
        if std::env::var_os(CHILD).is_some() {
            use reqwest::StatusCode;
            assert_eq!(cdn_status(StatusCode::OK, None), Ok(true));
            assert_eq!(cdn_status(StatusCode::NOT_FOUND, None), Ok(false));
            // A server error ends the poster chain but starts no cooldown.
            assert!(cdn_status(StatusCode::SERVICE_UNAVAILABLE, None).is_err());
            assert!(crate::polite::cooldown().is_ok());
            // A 429 from the CDN pauses everything, the CDN included, with no request sent:
            // this client would fail differently if it reached the network.
            assert!(cdn_status(StatusCode::TOO_MANY_REQUESTS, None).is_err());
            let client = Client {
                http: Err(Error::Network("the network must not be reached".into())),
                fixtures: None,
            };
            let url = "https://i.ytimg.com/vi/abcdefghijk/hq720.jpg";
            let err = try_runtime()
                .unwrap()
                .block_on(client.get_bytes_if_present(url))
                .unwrap_err();
            assert!(err.to_string().contains("pausing"), "{err}");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client::tests::image_cdn_respects_the_cooldown",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn logged_out_home_skips_the_empty_feed_request() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = Fixtures::new(dir.path().into(), crate::fixtures::Mode::Replay);
        let asked = fixtures.asked.clone();
        let client = Client {
            http: Err(Error::Network("no network in tests".into())),
            fixtures: Some(fixtures),
        };
        // Nothing is recorded, so every topic fails; what matters is what was asked for.
        assert!(client.home_feed().wait().is_err());
        let asked = asked.lock().unwrap();
        assert!(
            asked.iter().all(|k| !k.contains("FEwhat_to_watch")),
            "{asked:?}"
        );
        assert_eq!(asked.len(), ALL_MIX.len(), "{asked:?}");
        assert!(
            asked.iter().all(|k| k.starts_with("POST search ")),
            "{asked:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retries_only_transient_continuation_errors() {
        let started = tokio::time::Instant::now();
        let mut attempts = 0;
        let result = retry_page(true, || {
            attempts += 1;
            std::future::ready(if attempts == 3 {
                Ok(42)
            } else {
                Err(RequestFailure {
                    error: Error::Network("HTTP 503".into()),
                    retry: true,
                })
            })
        })
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(started.elapsed().as_millis(), 450);
        let mut attempts = 0;
        let denied = retry_page(true, || {
            attempts += 1;
            std::future::ready(Err::<(), _>(RequestFailure {
                error: Error::Network("HTTP 403".into()),
                retry: false,
            }))
        })
        .await;
        assert!(denied.is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn canonical_json_ignores_insertion_order() {
        let a: Value =
            serde_json::from_str(r#"{"query":"x","params":"p","n":[{"b":1,"a":2}]}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"params":"p","n":[{"a":2,"b":1}],"query":"x"}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            canonical_json(&a),
            r#"{"n":[{"a":2,"b":1}],"params":"p","query":"x"}"#
        );
    }
}
