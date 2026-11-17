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
    fn spawn<F>(fut: F) -> Job<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        runtime().spawn(async move {
            let _ = tx.send(fut.await);
        });
        Job { rx }
    }

    pub(crate) fn from_receiver(rx: oneshot::Receiver<Result<T>>) -> Job<T> {
        Job { rx }
    }

    /// A job that has already finished.
    pub fn ready(value: Result<T>) -> Job<T> {
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
        runtime().spawn(async move { f(self.await) });
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

/// The core's runtime: two worker threads plus a blocking pool for decoding.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_name("nt-core")
            .enable_all()
            .build()
            .expect("failed to start the nt-core runtime")
    })
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
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
            .expect("failed to build the HTTP client");
        Client {
            http,
            fixtures: Fixtures::from_env(),
        }
    }

    pub(crate) async fn get_bytes(&self, url: &str) -> Result<bytes::Bytes> {
        let key = format!("GET {url}");
        if let Some(f) = &self.fixtures
            && let Some(hit) = f.lookup(&key)?
        {
            return Ok(hit);
        }
        // Static image CDNs aren't YouTube's API; everything else waits its turn at the gate.
        let is_cdn = ["ytimg.com/", "ggpht.com/", "googleusercontent.com/"]
            .iter()
            .any(|h| url.contains(h));
        let _turn = if is_cdn {
            None
        } else {
            Some(crate::polite::turn().await?)
        };
        let resp = self.http.get(url).send().await?;
        if is_cdn {
            if !resp.status().is_success() {
                return Err(Error::Network(format!("{} for {url}", resp.status())));
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
        Ok(body)
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
        let _turn = crate::polite::turn().await?;
        let resp = self
            .http
            .post(url)
            .header("Origin", "https://www.youtube.com")
            .header("X-Youtube-Client-Name", "1")
            .header("X-Youtube-Client-Version", innertube::CLIENT_VERSION)
            .json(&body)
            .send()
            .await?;
        crate::polite::check_status(
            resp.status(),
            resp.headers().get(reqwest::header::RETRY_AFTER),
        )?;
        let raw = resp.bytes().await?;
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

    async fn more_raw(&self, c: Continuation) -> Result<Page<Video>> {
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
    pub fn home(&self, topic: &str) -> Job<Page<Video>> {
        let (this, topic) = (self.clone(), topic.to_string());
        Job::spawn(async move { this.home_raw(topic).await })
    }

    /// Video search results.
    pub fn search(&self, query: &str) -> Job<Page<Video>> {
        let (this, query) = (self.clone(), query.trim().to_string());
        Job::spawn(async move { this.search_raw(query, SEARCH_VIDEOS_ONLY).await })
    }

    /// The next page of any video list (home, search, related, channel).
    pub fn more_videos(&self, next: &Continuation) -> Job<Page<Video>> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move { this.more_raw(next).await })
    }

    /// Title, channel, description, likes, related videos and the comments cursor.
    pub fn watch(&self, video_id: &str) -> Job<WatchInfo> {
        let (this, id) = (self.clone(), video_id.to_string());
        Job::spawn(async move {
            let v = this.post("next", json!({ "videoId": id })).await?;
            innertube::parse_watch(&v, &id)
        })
    }

    /// A page of top comments. Pass [`WatchInfo::comments`] first, then each page's `next`.
    pub fn more_comments(&self, next: &Continuation) -> Job<CommentPage> {
        let (this, next) = (self.clone(), next.clone());
        Job::spawn(async move {
            let v = this
                .post("next", json!({ "continuation": next.token }))
                .await?;
            innertube::parse_comments(&v)
        })
    }

    /// A channel's header and the first page of its uploads.
    pub fn channel(&self, channel_id: &str) -> Job<ChannelInfo> {
        let (this, id) = (self.clone(), channel_id.to_string());
        Job::spawn(async move {
            let v = this
                .post(
                    "browse",
                    json!({ "browseId": id, "params": innertube::CHANNEL_VIDEOS_TAB }),
                )
                .await?;
            innertube::parse_channel(&v, &id)
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

pub(crate) fn urlencode(s: &str) -> String {
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
