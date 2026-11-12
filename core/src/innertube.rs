//! Parsers for YouTube's Innertube JSON (WEB client). Pure functions over
//! `serde_json::Value`, so they can be tested against recorded responses.

use crate::error::{Error, Result};
use crate::model::*;
use serde_json::Value;

pub(crate) const CLIENT_VERSION: &str = "2.20261105.01.00";
pub(crate) const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// Search filter: videos only (no channels, playlists or Shorts shelves).
pub(crate) const SEARCH_VIDEOS_ONLY: &str = "EgIQAQ%3D%3D";
/// Channel tab: Videos.
pub(crate) const CHANNEL_VIDEOS_TAB: &str = "EgZ2aWRlb3PyBgQKAjoA";

pub(crate) fn context() -> Value {
    serde_json::json!({
        "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en", "gl": "US" }
    })
}

/// Text from the three shapes Innertube uses: `simpleText`, `runs[].text`, `content`.
fn text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    if let Some(s) = v.get("content").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    let runs = v.get("runs")?.as_array()?;
    let joined: String = runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect();
    Some(joined).filter(|s| !s.is_empty())
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = match key.parse::<usize>() {
            Ok(i) if cur.is_array() => cur.get(i)?,
            _ => cur.get(*key)?,
        };
    }
    cur.as_str()
}

fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = match key.parse::<usize>() {
            Ok(i) if cur.is_array() => cur.get(i)?,
            _ => cur.get(*key)?,
        };
    }
    Some(cur)
}

/// Depth-first search for the first object holding `key`.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            if let Some(hit) = map.get(key) {
                return Some(hit);
            }
            map.values().find_map(|child| find(child, key))
        }
        Value::Array(items) => items.iter().find_map(|child| find(child, key)),
        _ => None,
    }
}

fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == key {
                    out.push(child);
                } else {
                    find_all(child, key, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|child| find_all(child, key, out)),
        _ => {}
    }
}

/// Avatar URLs come as protocol-relative or with sizes we don't need; make them absolute.
fn fix_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") { format!("https://{rest}") } else { url.to_string() }
}

fn video_renderer(v: &Value) -> Option<Video> {
    let id = v.get("videoId")?.as_str()?.to_string();
    let title = text(v.get("title")?)?;
    let byline = v.get("ownerText").or_else(|| v.get("longBylineText")).or_else(|| v.get("shortBylineText"));
    let channel = byline.and_then(text).unwrap_or_default();
    let channel_id = byline
        .and_then(|b| str_at(b, &["runs", "0", "navigationEndpoint", "browseEndpoint", "browseId"]))
        .map(str::to_string);
    let channel_avatar = str_at(
        v,
        &["channelThumbnailSupportedRenderers", "channelThumbnailWithLinkRenderer", "thumbnail", "thumbnails", "0", "url"],
    )
    .map(fix_url);
    // Grid items (channel shelves inside search) keep the time in a thumbnail overlay instead.
    let duration = v.get("lengthText").and_then(text).or_else(|| {
        v.get("thumbnailOverlays")?.as_array()?.iter().find_map(|o| {
            let ts = o.get("thumbnailOverlayTimeStatusRenderer")?;
            (ts.get("style").and_then(Value::as_str) != Some("LIVE")).then(|| ts.get("text").and_then(text))?
        })
    });
    let views = v.get("shortViewCountText").and_then(text).or_else(|| v.get("viewCountText").and_then(text));
    let published = v.get("publishedTimeText").and_then(text);
    let live_badge = v
        .get("badges")
        .and_then(Value::as_array)
        .is_some_and(|b| b.iter().any(|x| str_at(x, &["metadataBadgeRenderer", "style"]) == Some("BADGE_STYLE_TYPE_LIVE_NOW")));
    let is_live = live_badge || (duration.is_none() && views.as_deref().is_some_and(|s| s.contains("watching")));
    let snippet = at(v, &["detailedMetadataSnippets", "0", "snippetText"]).and_then(text);
    Some(Video {
        thumbnail: Video::thumbnail_url(&id),
        id,
        title,
        channel,
        channel_id,
        channel_avatar,
        duration,
        views,
        published,
        is_live,
        snippet,
    })
}

fn looks_like_count(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | 'K' | 'M' | 'B'))
}

/// The newer "lockup" card used by related videos and channel grids.
fn lockup(v: &Value) -> Option<Video> {
    if v.get("contentType")?.as_str()? != "LOCKUP_CONTENT_TYPE_VIDEO" {
        return None;
    }
    let id = v.get("contentId")?.as_str()?.to_string();
    let meta = at(v, &["metadata", "lockupMetadataViewModel"])?;
    let title = str_at(meta, &["title", "content"])?.to_string();

    let mut badges = Vec::new();
    if let Some(image) = v.get("contentImage") {
        find_all(image, "thumbnailBadgeViewModel", &mut badges);
    }
    let badge_text: Vec<&str> = badges.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
    let duration = badge_text.iter().find(|t| t.contains(':')).map(|t| t.to_string());
    let mut is_live = badge_text.iter().any(|t| t.eq_ignore_ascii_case("live"));

    let mut channel = String::new();
    let mut views = None;
    let mut published = None;
    if let Some(rows) = at(meta, &["metadata", "contentMetadataViewModel", "metadataRows"]).and_then(Value::as_array) {
        for row in rows {
            let Some(parts) = row.get("metadataParts").and_then(Value::as_array) else { continue };
            for part in parts {
                let Some(t) = str_at(part, &["text", "content"]) else { continue };
                let lower = t.to_ascii_lowercase();
                if lower.contains("watching") {
                    is_live = true;
                    views = Some(t.to_string());
                } else if lower.contains("view") {
                    views = Some(t.to_string());
                } else if looks_like_count(t) {
                    views = Some(format!("{t} views"));
                } else if lower.contains("ago") || lower.starts_with("streamed") || lower.starts_with("premiere") {
                    published = Some(t.to_string());
                } else if channel.is_empty() {
                    channel = t.to_string();
                }
            }
        }
    }
    let avatar = at(meta, &["image", "decoratedAvatarViewModel"]);
    let channel_avatar =
        avatar.and_then(|a| str_at(a, &["avatar", "avatarViewModel", "image", "sources", "0", "url"])).map(fix_url);
    let channel_id = avatar
        .and_then(|a| at(a, &["rendererContext", "commandContext", "onTap", "innertubeCommand", "browseEndpoint", "browseId"]))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(Video {
        thumbnail: Video::thumbnail_url(&id),
        id,
        title,
        channel,
        channel_id,
        channel_avatar,
        duration,
        views,
        published,
        is_live,
        snippet: None,
    })
}

/// Collect every video card under `v`, in document order, without duplicates.
fn collect_videos(v: &Value, out: &mut Vec<Video>) {
    match v {
        Value::Object(map) => {
            for (key, child) in map {
                let parsed = match key.as_str() {
                    "videoRenderer" | "compactVideoRenderer" | "gridVideoRenderer" => video_renderer(child),
                    "lockupViewModel" => lockup(child),
                    // Shorts shelves, ads and end screens aren't feed items.
                    "reelShelfRenderer" | "shortsLockupViewModel" | "endScreenVideoRenderer" | "adSlotRenderer"
                    | "playerOverlays" | "engagementPanels" => continue,
                    _ => {
                        collect_videos(child, out);
                        continue;
                    }
                };
                if let Some(video) = parsed
                    && !out.iter().any(|x| x.id == video.id)
                {
                    out.push(video);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|child| collect_videos(child, out)),
        _ => {}
    }
}

/// The token of the last "load more" marker under `v`.
fn continuation_token(v: &Value) -> Option<String> {
    let mut items = Vec::new();
    find_all(v, "continuationItemRenderer", &mut items);
    items.iter().rev().find_map(|item| {
        str_at(item, &["continuationEndpoint", "continuationCommand", "token"])
            .or_else(|| str_at(item, &["button", "buttonRenderer", "command", "continuationCommand", "token"]))
            .map(str::to_string)
    })
}

fn page(v: &Value, endpoint: &str) -> Page<Video> {
    let mut items = Vec::new();
    collect_videos(v, &mut items);
    let next = continuation_token(v).map(|token| Continuation { endpoint: endpoint.to_string(), token, owner: None });
    Page { items, next }
}

pub(crate) fn parse_search(v: &Value) -> Result<Page<Video>> {
    let root = at(v, &["contents", "twoColumnSearchResultsRenderer", "primaryContents"])
        .ok_or_else(|| Error::Parse("search: no primaryContents".into()))?;
    Ok(page(root, "search"))
}

/// Any "load more" answer: the items live under `onResponseReceived*`.
pub(crate) fn parse_video_continuation(v: &Value, endpoint: &str) -> Result<Page<Video>> {
    let mut roots = Vec::new();
    for key in ["onResponseReceivedCommands", "onResponseReceivedActions", "onResponseReceivedEndpoints"] {
        if let Some(root) = v.get(key) {
            roots.push(root);
        }
    }
    if roots.is_empty() {
        return Err(Error::Parse(format!("{endpoint} continuation: no onResponseReceived*")));
    }
    let mut items = Vec::new();
    let mut next = None;
    for root in roots {
        collect_videos(root, &mut items);
        if let Some(token) = continuation_token(root) {
            next = Some(Continuation { endpoint: endpoint.to_string(), token, owner: None });
        }
    }
    Ok(Page { items, next })
}

pub(crate) fn parse_watch(v: &Value, video_id: &str) -> Result<WatchInfo> {
    let two = at(v, &["contents", "twoColumnWatchNextResults"]).ok_or_else(|| Error::Parse("watch: no results".into()))?;
    let contents = at(two, &["results", "results", "contents"]).and_then(Value::as_array);
    let mut info = WatchInfo { id: video_id.to_string(), ..Default::default() };
    for item in contents.into_iter().flatten() {
        if let Some(primary) = item.get("videoPrimaryInfoRenderer") {
            info.title = primary.get("title").and_then(text).unwrap_or_default();
            info.views = at(primary, &["viewCount", "videoViewCountRenderer", "viewCount"]).and_then(text);
            info.date = primary.get("dateText").and_then(text);
            let mut likes = Vec::new();
            find_all(primary, "likeButtonViewModel", &mut likes);
            info.likes = likes.first().and_then(|l| find(l, "title")).and_then(Value::as_str).map(str::to_string);
        }
        if let Some(secondary) = item.get("videoSecondaryInfoRenderer") {
            let owner = at(secondary, &["owner", "videoOwnerRenderer"]);
            if let Some(owner) = owner {
                info.channel = ChannelRef {
                    id: str_at(owner, &["title", "runs", "0", "navigationEndpoint", "browseEndpoint", "browseId"])
                        .unwrap_or_default()
                        .to_string(),
                    name: owner.get("title").and_then(text).unwrap_or_default(),
                    avatar: at(owner, &["thumbnail", "thumbnails"])
                        .and_then(Value::as_array)
                        .and_then(|t| t.first())
                        .and_then(|t| t.get("url"))
                        .and_then(Value::as_str)
                        .map(fix_url),
                    subscribers: owner.get("subscriberCountText").and_then(text),
                };
            }
            info.description = str_at(secondary, &["attributedDescription", "content"])
                .map(str::to_string)
                .or_else(|| secondary.get("description").and_then(text))
                .unwrap_or_default();
        }
        if let Some(section) = item.get("itemSectionRenderer")
            && section.get("sectionIdentifier").and_then(Value::as_str) == Some("comment-item-section")
        {
            info.comments =
                continuation_token(section).map(|token| Continuation { endpoint: "next".into(), token, owner: None });
        }
    }
    if info.title.is_empty() {
        return Err(Error::Parse("watch: video is unavailable".into()));
    }
    if let Some(secondary) = at(two, &["secondaryResults", "secondaryResults"]) {
        info.related = page(secondary, "next");
    }
    Ok(info)
}

pub(crate) fn parse_comments(v: &Value) -> Result<CommentPage> {
    let mut page = CommentPage::default();
    let mut keys = Vec::new();
    let endpoints = v.get("onResponseReceivedEndpoints").ok_or_else(|| Error::Parse("comments: no endpoints".into()))?;
    let mut threads = Vec::new();
    find_all(endpoints, "commentThreadRenderer", &mut threads);
    for thread in threads {
        if let Some(key) = str_at(thread, &["commentViewModel", "commentViewModel", "commentKey"]) {
            keys.push(key);
        }
    }
    if let Some(header) = find(endpoints, "commentsHeaderRenderer") {
        page.total = str_at(header, &["countText", "runs", "0", "text"]).map(str::to_string);
    }
    // The body's "load more" marker is the last one; the header may hold sort menus with tokens of their own.
    for endpoint in endpoints.as_array().into_iter().flatten() {
        let Some(items) = endpoint
            .get("reloadContinuationItemsCommand")
            .or_else(|| endpoint.get("appendContinuationItemsAction"))
            .and_then(|c| c.get("continuationItems"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        if let Some(last) = items.last()
            && let Some(marker) = last.get("continuationItemRenderer")
            && let Some(token) = str_at(marker, &["continuationEndpoint", "continuationCommand", "token"])
        {
            page.next = Some(Continuation { endpoint: "next".into(), token: token.to_string(), owner: None });
        }
    }
    let mutations = at(v, &["frameworkUpdates", "entityBatchUpdate", "mutations"]).and_then(Value::as_array);
    let payloads: Vec<&Value> = mutations
        .into_iter()
        .flatten()
        .filter_map(|m| at(m, &["payload", "commentEntityPayload"]))
        .collect();
    for key in keys {
        let Some(p) = payloads.iter().find(|p| p.get("key").and_then(Value::as_str) == Some(key)) else { continue };
        let nonempty = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        page.items.push(Comment {
            id: str_at(p, &["properties", "commentId"]).unwrap_or_default().to_string(),
            author: str_at(p, &["author", "displayName"]).unwrap_or_default().to_string(),
            author_channel_id: nonempty(str_at(p, &["author", "channelId"])),
            author_avatar: str_at(p, &["author", "avatarThumbnailUrl"]).map(fix_url),
            text: str_at(p, &["properties", "content", "content"]).unwrap_or_default().to_string(),
            published: nonempty(str_at(p, &["properties", "publishedTime"])),
            likes: nonempty(str_at(p, &["toolbar", "likeCountNotliked"])),
            reply_count: nonempty(str_at(p, &["toolbar", "replyCount"])),
            by_creator: at(p, &["author", "isCreator"]).and_then(Value::as_bool).unwrap_or(false),
        });
    }
    Ok(page)
}

pub(crate) fn parse_channel(v: &Value, channel_id: &str) -> Result<ChannelInfo> {
    let meta = at(v, &["metadata", "channelMetadataRenderer"]).ok_or_else(|| Error::Parse("channel: no metadata".into()))?;
    let mut info = ChannelInfo {
        id: str_at(meta, &["externalId"]).unwrap_or(channel_id).to_string(),
        name: str_at(meta, &["title"]).unwrap_or_default().to_string(),
        description: str_at(meta, &["description"]).unwrap_or_default().to_string(),
        avatar: str_at(meta, &["avatar", "thumbnails", "0", "url"]).map(|u| resize_avatar(&fix_url(u), 176)),
        ..Default::default()
    };
    if let Some(header) = at(v, &["header", "pageHeaderRenderer", "content", "pageHeaderViewModel"]) {
        if let Some(banner) = header.get("banner") {
            let mut sources = Vec::new();
            find_all(banner, "sources", &mut sources);
            info.banner = sources.first().and_then(|s| str_at(s, &["0", "url"])).map(fix_url);
        }
        if let Some(rows) = at(header, &["metadata", "contentMetadataViewModel", "metadataRows"]).and_then(Value::as_array) {
            for row in rows {
                for part in row.get("metadataParts").and_then(Value::as_array).into_iter().flatten() {
                    let Some(t) = str_at(part, &["text", "content"]) else { continue };
                    if t.starts_with('@') {
                        info.handle = Some(t.to_string());
                    } else if t.contains("subscriber") {
                        info.subscribers = Some(t.to_string());
                    } else if t.contains("video") {
                        info.video_count = Some(t.to_string());
                    }
                }
            }
        }
    }
    let tabs = at(v, &["contents", "twoColumnBrowseResultsRenderer", "tabs"]).and_then(Value::as_array);
    let selected = tabs.into_iter().flatten().find_map(|tab| {
        let tab = tab.get("tabRenderer")?;
        tab.get("selected").and_then(Value::as_bool).filter(|s| *s)?;
        tab.get("content")
    });
    if let Some(content) = selected {
        info.videos = page(content, "browse");
    }
    if let Some(next) = &mut info.videos.next {
        next.owner = Some((info.id.clone(), info.name.clone(), info.avatar.clone()));
    }
    for video in &mut info.videos.items {
        video.channel = info.name.clone();
        video.channel_id = Some(info.id.clone());
        video.channel_avatar = info.avatar.clone();
    }
    Ok(info)
}

/// Rewrite a `yt3` avatar URL's size parameter (`=s900-…` to `=s176-…`).
pub(crate) fn resize_avatar(url: &str, size: u32) -> String {
    let Some(eq) = url.rfind("=s") else { return url.to_string() };
    let rest = &url[eq + 2..];
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return url.to_string();
    }
    format!("{}=s{}{}", &url[..eq], size, &rest[digits..])
}

/// `window.google.ac.h(["q",[["suggestion",0,[…]],…],…])`
pub(crate) fn parse_suggestions(body: &str) -> Vec<String> {
    let Some(start) = body.find('(') else { return Vec::new() };
    let Some(end) = body.rfind(')') else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&body[start + 1..end]) else { return Vec::new() };
    v.get(1)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get(0).and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// A channel's Atom feed: its fifteen newest uploads, with real timestamps.
pub(crate) fn parse_feed(xml: &str, now_unix: i64) -> Result<Vec<(i64, Video)>> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Parse(format!("feed: {e}")))?;
    let child_text = |node: roxmltree::Node, name: &str| {
        node.descendants().find(|n| n.tag_name().name() == name).and_then(|n| n.text()).map(str::to_string)
    };
    let mut out = Vec::new();
    for entry in doc.descendants().filter(|n| n.tag_name().name() == "entry") {
        let Some(id) = child_text(entry, "videoId") else { continue };
        let published = child_text(entry, "published").and_then(|s| parse_rfc3339(&s)).unwrap_or(0);
        let views = entry
            .descendants()
            .find(|n| n.tag_name().name() == "statistics")
            .and_then(|n| n.attribute("views"))
            .and_then(|v| v.parse::<u64>().ok());
        out.push((
            published,
            Video {
                thumbnail: Video::thumbnail_url(&id),
                title: child_text(entry, "title").unwrap_or_default(),
                channel: child_text(entry, "name").unwrap_or_default(),
                channel_id: child_text(entry, "channelId"),
                channel_avatar: None,
                duration: None,
                views: views.map(|v| format!("{} views", crate::format::short_count(v))),
                published: Some(crate::format::relative_time(now_unix - published)),
                is_live: false,
                snippet: None,
                id,
            },
        ));
    }
    Ok(out)
}

/// "2026-11-10T18:19:55+00:00" to Unix seconds. Offsets other than UTC are honoured.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut t = days * 86_400 + hh * 3600 + mm * 60 + ss;
    let tail = &s[19..];
    if let Some(pos) = tail.find(['+', '-']) {
        let sign = if tail.as_bytes()[pos] == b'+' { 1 } else { -1 };
        let off = &tail[pos + 1..];
        let oh = off.get(0..2).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let om = off.get(3..5).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        t -= sign * (oh * 3600 + om * 60);
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-10T18:19:55+00:00"), Some(1_794_334_795));
        assert_eq!(parse_rfc3339("2026-11-10T19:19:55+01:00"), Some(1_794_334_795));
    }

    #[test]
    fn avatar_resize() {
        assert_eq!(resize_avatar("https://x/abc=s900-c-k-no-rj", 176), "https://x/abc=s176-c-k-no-rj");
        assert_eq!(resize_avatar("https://x/abc", 176), "https://x/abc");
    }

    #[test]
    fn suggestions() {
        let body = r#"window.google.ac.h(["rust",[["rust programming",0,[512]],["rust game",0,[]]],{"k":1}])"#;
        assert_eq!(parse_suggestions(body), vec!["rust programming", "rust game"]);
    }

    #[test]
    fn counts() {
        assert!(looks_like_count("6.1B"));
        assert!(looks_like_count("1,234"));
        assert!(!looks_like_count("officialpsy"));
        assert!(!looks_like_count("14y ago"));
    }
}
