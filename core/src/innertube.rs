//! Parsers for YouTube's Innertube JSON (WEB client). Pure functions over
//! `serde_json::Value`, so they can be tested against recorded responses.

use crate::error::{Error, Result};
use crate::model::*;
use serde_json::Value;

pub(crate) const CLIENT_VERSION: &str = "2.20261105.01.00";
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
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
pub(crate) fn text(v: &Value) -> Option<String> {
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
    let joined: String = runs
        .iter()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect();
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
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

fn largest_image(list: &Value) -> Option<String> {
    list.as_array()?
        .iter()
        .filter_map(|image| Some((image["width"].as_u64().unwrap_or(0), image["url"].as_str()?)))
        .max_by_key(|(width, _)| *width)
        .map(|(_, url)| fix_url(url))
}

fn video_thumbnail(id: &str, images: &Value) -> String {
    images
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| image["url"].as_str())
        .rfind(|url| url.contains("/hq720."))
        .map(fix_url)
        .unwrap_or_else(|| Video::thumbnail_url(id))
}

/// A verified or official-artist mark: a badge style or an inline icon next to the name.
fn verified_mark(v: &Value) -> bool {
    let mut styles = Vec::new();
    find_all(v, "style", &mut styles);
    let mut icons = Vec::new();
    find_all(v, "imageName", &mut icons);
    styles.iter().chain(icons.iter()).any(|s| {
        matches!(
            s.as_str(),
            Some(
                "BADGE_STYLE_TYPE_VERIFIED"
                    | "BADGE_STYLE_TYPE_VERIFIED_ARTIST"
                    | "CHECK_CIRCLE_FILLED"
                    | "CHECK_CIRCLE_THICK"
                    | "AUDIO_BADGE"
            )
        )
    })
}

/// "Premieres in 3 hours" from a start time and YouTube's text with `DATE_PLACEHOLDER`.
fn upcoming_text(event: &Value, now_unix: i64) -> Option<String> {
    let start = event
        .get("startTime")
        .and_then(|t| t.as_str()?.parse::<i64>().ok().or_else(|| t.as_i64()))?;
    let left = (start - now_unix).max(0);
    let when = match left {
        0..60 => "in a moment".to_string(),
        60..3600 => plural(left / 60, "minute"),
        3600..86_400 => plural(left / 3600, "hour"),
        _ => plural(left / 86_400, "day"),
    };
    let template = event
        .get("upcomingEventText")
        .and_then(text)
        .unwrap_or_else(|| "Premieres DATE_PLACEHOLDER".into());
    Some(if template.contains("DATE_PLACEHOLDER") {
        template.replace("DATE_PLACEHOLDER", &when)
    } else {
        format!("{template} {when}")
    })
}

fn plural(n: i64, unit: &str) -> String {
    format!("in {n} {unit}{}", if n == 1 { "" } else { "s" })
}

fn unix_now() -> i64 {
    crate::format::unix_now()
}

fn video_renderer(v: &Value) -> Option<Video> {
    let id = v
        .get("videoId")?
        .as_str()
        .filter(|id| !id.is_empty())?
        .to_string();
    let title = text(v.get("title")?)?;
    let byline = v
        .get("ownerText")
        .or_else(|| v.get("longBylineText"))
        .or_else(|| v.get("shortBylineText"));
    let channel = byline.and_then(text).unwrap_or_default();
    let channel_id = byline
        .and_then(|b| {
            str_at(
                b,
                &[
                    "runs",
                    "0",
                    "navigationEndpoint",
                    "browseEndpoint",
                    "browseId",
                ],
            )
        })
        .map(str::to_string);
    let channel_avatar = largest_image(
        &v["channelThumbnailSupportedRenderers"]["channelThumbnailWithLinkRenderer"]["thumbnail"]["thumbnails"],
    );
    // Grid items (channel shelves inside search) keep the time in a thumbnail overlay instead.
    let duration = v.get("lengthText").and_then(text).or_else(|| {
        v.get("thumbnailOverlays")?
            .as_array()?
            .iter()
            .find_map(|o| {
                let ts = o.get("thumbnailOverlayTimeStatusRenderer")?;
                (ts.get("style").and_then(Value::as_str) != Some("LIVE"))
                    .then(|| ts.get("text").and_then(text))?
            })
    });
    let views = v
        .get("shortViewCountText")
        .and_then(text)
        .or_else(|| v.get("viewCountText").and_then(text));
    let published = v.get("publishedTimeText").and_then(text);
    let live_badge = v.get("badges").and_then(Value::as_array).is_some_and(|b| {
        b.iter().any(|x| {
            str_at(x, &["metadataBadgeRenderer", "style"]) == Some("BADGE_STYLE_TYPE_LIVE_NOW")
        })
    });
    let is_live = live_badge
        || (duration.is_none() && views.as_deref().is_some_and(|s| s.contains("watching")));
    let snippet = at(v, &["detailedMetadataSnippets", "0", "snippetText"]).and_then(text);
    let badges = v
        .get("badges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| text(&b["metadataBadgeRenderer"]["label"]))
        .collect();
    let channel_verified = v.get("ownerBadges").is_some_and(verified_mark);
    let upcoming = v
        .get("upcomingEventData")
        .and_then(|e| upcoming_text(e, unix_now()));
    Some(Video {
        badges,
        channel_verified,
        upcoming,
        thumbnail: video_thumbnail(&id, &v["thumbnail"]["thumbnails"]),
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
        moment: None,
    })
}

fn looks_like_count(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | 'K' | 'M' | 'B'))
}

/// The newer "lockup" card used by related videos and channel grids.
fn lockup(v: &Value) -> Option<Video> {
    if v.get("contentType")?.as_str()? != "LOCKUP_CONTENT_TYPE_VIDEO" {
        return None;
    }
    let id = v
        .get("contentId")?
        .as_str()
        .filter(|id| !id.is_empty())?
        .to_string();
    let meta = at(v, &["metadata", "lockupMetadataViewModel"])?;
    let title = str_at(meta, &["title", "content"])?.to_string();

    let mut badges = Vec::new();
    if let Some(image) = v.get("contentImage") {
        find_all(image, "thumbnailBadgeViewModel", &mut badges);
    }
    let badge_text: Vec<&str> = badges
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    let duration = badge_text
        .iter()
        .find(|t| t.contains(':'))
        .map(|t| t.to_string());
    let mut is_live = badge_text.iter().any(|t| t.eq_ignore_ascii_case("live"));

    let mut channel = String::new();
    let mut channel_verified = false;
    let mut views = None;
    let mut published = None;
    let mut upcoming = None;
    let mut card_badges = Vec::new();
    if let Some(rows) = at(
        meta,
        &["metadata", "contentMetadataViewModel", "metadataRows"],
    )
    .and_then(Value::as_array)
    {
        for row in rows {
            for badge in row
                .get("badges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(t) = str_at(badge, &["badgeViewModel", "badgeText"]) {
                    card_badges.push(t.to_string());
                }
            }
            let Some(parts) = row.get("metadataParts").and_then(Value::as_array) else {
                continue;
            };
            for part in parts {
                let Some(t) = str_at(part, &["text", "content"]) else {
                    continue;
                };
                let lower = t.to_ascii_lowercase();
                if lower.starts_with("premieres") || lower.starts_with("scheduled for") {
                    upcoming = Some(t.to_string());
                }
                if lower.contains("watching") {
                    is_live = true;
                    views = Some(t.to_string());
                } else if lower.contains("view") {
                    views = Some(t.to_string());
                } else if looks_like_count(t) {
                    views = Some(format!("{t} views"));
                } else if lower.contains("ago")
                    || lower.starts_with("streamed")
                    || lower.starts_with("premiere")
                {
                    published = Some(t.to_string());
                } else if channel.is_empty() {
                    channel = t.to_string();
                    channel_verified = verified_mark(&part["text"]);
                }
            }
        }
    }
    let avatar = at(meta, &["image", "decoratedAvatarViewModel"]);
    let channel_avatar =
        avatar.and_then(|a| largest_image(&a["avatar"]["avatarViewModel"]["image"]["sources"]));
    let channel_id = avatar
        .and_then(|a| {
            at(
                a,
                &[
                    "rendererContext",
                    "commandContext",
                    "onTap",
                    "innertubeCommand",
                    "browseEndpoint",
                    "browseId",
                ],
            )
        })
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(Video {
        thumbnail: video_thumbnail(
            &id,
            find(&v["contentImage"], "sources").unwrap_or(&Value::Null),
        ),
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
        moment: None,
        badges: card_badges,
        channel_verified,
        upcoming,
    })
}

/// Collect every video card under `v`, in document order, without duplicates.
fn collect_videos(v: &Value, out: &mut Vec<Video>) {
    match v {
        Value::Object(map) => {
            for (key, child) in map {
                let parsed = match key.as_str() {
                    "videoRenderer" | "compactVideoRenderer" | "gridVideoRenderer" => {
                        video_renderer(child)
                    }
                    "lockupViewModel" => lockup(child),
                    // Shorts shelves, ads and end screens aren't feed items.
                    "reelShelfRenderer"
                    | "shortsLockupViewModel"
                    | "endScreenVideoRenderer"
                    | "adSlotRenderer"
                    | "playerOverlays"
                    | "engagementPanels" => continue,
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
        str_at(
            item,
            &["continuationEndpoint", "continuationCommand", "token"],
        )
        .or_else(|| {
            str_at(
                item,
                &[
                    "button",
                    "buttonRenderer",
                    "command",
                    "continuationCommand",
                    "token",
                ],
            )
        })
        .map(str::to_string)
    })
}

fn page(v: &Value, endpoint: &str) -> Page<Video> {
    let mut items = Vec::new();
    collect_videos(v, &mut items);
    let next = continuation_token(v).map(|token| Continuation {
        endpoint: endpoint.to_string(),
        token,
        owner: None,
    });
    Page { items, next }
}

pub(crate) fn parse_search(v: &Value) -> Result<Page<Video>> {
    let root = at(
        v,
        &[
            "contents",
            "twoColumnSearchResultsRenderer",
            "primaryContents",
        ],
    )
    .ok_or_else(|| Error::Parse("search: no primaryContents".into()))?;
    Ok(page(root, "search"))
}

pub(crate) fn search_page(v: &Value, query: &str, continuation: bool) -> Result<SearchPage> {
    let results = if continuation {
        parse_video_continuation(v, "search")?
    } else {
        parse_search(v)?
    };
    let mut entries = Vec::new();
    if continuation {
        for key in [
            "onResponseReceivedCommands",
            "onResponseReceivedActions",
            "onResponseReceivedEndpoints",
        ] {
            collect_search(&v[key], &mut entries);
        }
    } else {
        collect_search(
            &v["contents"]["twoColumnSearchResultsRenderer"]["primaryContents"],
            &mut entries,
        );
    }
    let notices = if continuation {
        Vec::new()
    } else {
        search_notices(v, query, entries.is_empty())
    };
    let mut search_chips: Vec<_> = chips(v, "search")
        .into_iter()
        .filter(|chip| !chip.label.eq_ignore_ascii_case("shorts"))
        .map(|chip| SearchOption {
            filter: chip.next.map(SearchFilter::Chip).or_else(|| {
                chip.label
                    .eq_ignore_ascii_case("all")
                    .then_some(SearchFilter::All)
            }),
            label: chip.label,
            selected: chip.selected,
        })
        .collect();
    if !continuation {
        search_chips.push(SearchOption {
            label: "Transcripts".into(),
            selected: false,
            filter: Some(SearchFilter::Transcripts),
        });
    }
    let mut groups = Vec::new();
    find_all(v, "searchFilterGroupRenderer", &mut groups);
    let filters = groups
        .iter()
        .filter_map(|group| {
            let title = text(&group["title"])?;
            let options = group["filters"]
                .as_array()?
                .iter()
                .filter_map(|entry| {
                    let f = &entry["searchFilterRenderer"];
                    let label = text(&f["label"])?;
                    if label.eq_ignore_ascii_case("shorts") {
                        return None;
                    }
                    let status = f["status"].as_str();
                    let filter = if status == Some("FILTER_STATUS_DISABLED") {
                        None
                    } else {
                        f["navigationEndpoint"]["searchEndpoint"]["params"]
                            .as_str()
                            .map(|p| SearchFilter::Params(p.to_owned()))
                            .or_else(|| {
                                f["navigationEndpoint"]["searchEndpoint"]
                                    .is_object()
                                    .then_some(SearchFilter::All)
                            })
                    };
                    Some(SearchOption {
                        label,
                        selected: status == Some("FILTER_STATUS_SELECTED"),
                        filter,
                    })
                })
                .collect();
            Some(SearchFilterGroup { title, options })
        })
        .collect();
    Ok(SearchPage {
        results,
        notices,
        chips: search_chips,
        filters,
        entries,
        ..Default::default()
    })
}

fn collect_search(v: &Value, out: &mut Vec<SearchResult>) {
    match v {
        Value::Array(values) => values.iter().for_each(|v| collect_search(v, out)),
        Value::Object(values) => {
            for (key, value) in values {
                let found = match key.as_str() {
                    "videoRenderer" | "gridVideoRenderer" | "compactVideoRenderer" => {
                        video_renderer(value).map(Box::new).map(SearchResult::Video)
                    }
                    "lockupViewModel" => lockup(value)
                        .map(Box::new)
                        .map(SearchResult::Video)
                        .or_else(|| playlist_lockup(value).map(SearchResult::Playlist)),
                    "playlistRenderer" | "gridPlaylistRenderer" => {
                        playlist_renderer(value).map(SearchResult::Playlist)
                    }
                    "channelRenderer" => search_channel(value).map(SearchResult::Channel),
                    "shelfRenderer" => search_shelf(value).map(SearchResult::Shelf),
                    "reelShelfRenderer" | "shortsLockupViewModel" | "adSlotRenderer" => None,
                    _ => {
                        collect_search(value, out);
                        None
                    }
                };
                if let Some(found) = found {
                    out.push(found);
                }
            }
        }
        _ => {}
    }
}

fn search_channel(v: &Value) -> Option<SearchChannel> {
    let subscribers = text(&v["subscriberCountText"]).filter(|s| !s.starts_with('@'));
    let count_or_handle = text(&v["videoCountText"]);
    let handle = text(&v["subscriberCountText"])
        .filter(|s| s.starts_with('@'))
        .or_else(|| count_or_handle.clone().filter(|s| s.starts_with('@')))
        .or_else(|| {
            v["navigationEndpoint"]["commandMetadata"]["webCommandMetadata"]["url"]
                .as_str()
                .and_then(|s| s.strip_prefix("/@"))
                .map(|s| format!("@{s}"))
        });
    Some(SearchChannel {
        channel: ChannelRef {
            id: v["channelId"].as_str()?.to_owned(),
            name: text(&v["title"])?,
            avatar: largest_image(&v["thumbnail"]["thumbnails"]),
            subscribers: subscribers
                .or_else(|| count_or_handle.filter(|s| s.contains("subscriber"))),
        },
        handle,
        verified: verified_mark(&v["ownerBadges"]),
    })
}

fn playlist_renderer(v: &Value) -> Option<PlaylistRef> {
    let mut thumbnails = Vec::new();
    find_all(v, "thumbnails", &mut thumbnails);
    Some(PlaylistRef {
        id: v["playlistId"].as_str()?.to_owned(),
        title: text(&v["title"])?,
        thumbnail: thumbnails.into_iter().find_map(largest_image),
        video_count: text(&v["videoCountText"])
            .or_else(|| text(&v["videoCountShortText"]))
            .or_else(|| v["videoCount"].as_u64().map(|n| n.to_string())),
    })
}

fn playlist_lockup(v: &Value) -> Option<PlaylistRef> {
    if !matches!(
        v["contentType"].as_str()?,
        "LOCKUP_CONTENT_TYPE_PLAYLIST" | "LOCKUP_CONTENT_TYPE_RADIO"
    ) {
        return None;
    }
    let mut badges = Vec::new();
    find_all(&v["contentImage"], "thumbnailBadgeViewModel", &mut badges);
    let video_count = badges
        .iter()
        .find_map(|b| text(&b["text"]).filter(|t| t.contains("video")));
    let video_count = video_count.or_else(|| {
        let mut parts = Vec::new();
        find_all(&v["metadata"], "text", &mut parts);
        parts.into_iter().find_map(|p| {
            text(p).filter(|t| t.contains("video") && t.bytes().any(|c| c.is_ascii_digit()))
        })
    });
    Some(PlaylistRef {
        id: v["contentId"].as_str()?.to_owned(),
        title: text(&v["metadata"]["lockupMetadataViewModel"]["title"])?,
        thumbnail: find(&v["contentImage"], "sources").and_then(largest_image),
        video_count,
    })
}

fn search_shelf(v: &Value) -> Option<SearchShelf> {
    let mut items = Vec::new();
    collect_search(&v["content"], &mut items);
    (!items.is_empty()).then_some(SearchShelf {
        title: text(&v["title"]).unwrap_or_default(),
        items,
    })
}

fn search_notices(v: &Value, query: &str, empty: bool) -> Vec<SearchNotice> {
    let root = &v["contents"]["twoColumnSearchResultsRenderer"]["primaryContents"];
    let mut notices = Vec::new();
    if let Some(promo) = find(root, "backgroundPromoRenderer")
        && let Some(title) = text(&promo["title"])
        && let Some(body) = text(&promo["bodyText"])
    {
        let age = title.to_ascii_lowercase().contains("age")
            || body.to_ascii_lowercase().contains("inappropriate");
        if age {
            notices.push(SearchNotice::AgeGated { title, body });
        }
    }
    if let Some(renderer) = find(root, "showingResultsForRenderer")
        && let Some(showing) = text(&renderer["correctedQuery"])
    {
        let original = text(&renderer["originalQuery"]).unwrap_or_else(|| query.to_owned());
        notices.push(SearchNotice::Corrected { showing, original });
    }
    if let Some(renderer) = find(root, "didYouMeanRenderer")
        && let Some(suggestion) = text(&renderer["correctedQuery"])
    {
        notices.push(SearchNotice::DidYouMean { suggestion });
    }
    if empty
        && !notices
            .iter()
            .any(|n| matches!(n, SearchNotice::AgeGated { .. }))
    {
        notices.push(SearchNotice::NoResults);
    }
    notices
}

/// Any "load more" answer: the items live under `onResponseReceived*`.
pub(crate) fn parse_video_continuation(v: &Value, endpoint: &str) -> Result<Page<Video>> {
    let mut roots = Vec::new();
    for key in [
        "onResponseReceivedCommands",
        "onResponseReceivedActions",
        "onResponseReceivedEndpoints",
    ] {
        if let Some(root) = v.get(key) {
            roots.push(root);
        }
    }
    if roots.is_empty() {
        return Err(Error::Parse(format!(
            "{endpoint} continuation: no onResponseReceived*"
        )));
    }
    let mut items = Vec::new();
    let mut next = None;
    for root in roots {
        collect_videos(root, &mut items);
        if let Some(token) = continuation_token(root) {
            next = Some(Continuation {
                endpoint: endpoint.to_string(),
                token,
                owner: None,
            });
        }
    }
    Ok(Page { items, next })
}

pub(crate) fn parse_watch(v: &Value, video_id: &str) -> Result<WatchInfo> {
    let two = at(v, &["contents", "twoColumnWatchNextResults"])
        .ok_or_else(|| Error::Parse("watch: no results".into()))?;
    let contents = at(two, &["results", "results", "contents"]).and_then(Value::as_array);
    let mut info = WatchInfo {
        id: video_id.to_string(),
        ..Default::default()
    };
    for item in contents.into_iter().flatten() {
        if let Some(primary) = item.get("videoPrimaryInfoRenderer") {
            info.title = primary.get("title").and_then(text).unwrap_or_default();
            info.views = at(
                primary,
                &["viewCount", "videoViewCountRenderer", "viewCount"],
            )
            .and_then(text);
            info.date = primary.get("dateText").and_then(text);
            let mut likes = Vec::new();
            find_all(primary, "likeButtonViewModel", &mut likes);
            info.likes = likes
                .first()
                .and_then(|l| find(l, "title"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if let Some(secondary) = item.get("videoSecondaryInfoRenderer") {
            let owner = at(secondary, &["owner", "videoOwnerRenderer"]);
            if let Some(owner) = owner {
                info.channel = ChannelRef {
                    id: str_at(
                        owner,
                        &[
                            "title",
                            "runs",
                            "0",
                            "navigationEndpoint",
                            "browseEndpoint",
                            "browseId",
                        ],
                    )
                    .unwrap_or_default()
                    .to_string(),
                    name: owner.get("title").and_then(text).unwrap_or_default(),
                    avatar: largest_image(&owner["thumbnail"]["thumbnails"]),
                    subscribers: owner.get("subscriberCountText").and_then(text),
                };
                info.channel_verified = owner.get("badges").is_some_and(verified_mark);
            }
            info.description = str_at(secondary, &["attributedDescription", "content"])
                .map(str::to_string)
                .or_else(|| secondary.get("description").and_then(text))
                .unwrap_or_default();
        }
        if let Some(section) = item.get("itemSectionRenderer")
            && section.get("sectionIdentifier").and_then(Value::as_str)
                == Some("comment-item-section")
        {
            info.comments = continuation_token(section).map(|token| Continuation {
                endpoint: "next".into(),
                token,
                owner: None,
            });
        }
    }
    if info.title.is_empty() {
        return Err(Error::Parse("watch: video is unavailable".into()));
    }
    if let Some(secondary) = at(two, &["secondaryResults", "secondaryResults"]) {
        info.related = page(secondary, "next");
        info.related_chips = chips(secondary, "next");
    }
    info.playlist = playlist_panel(v, None, Some(video_id));
    info.chapters = chapters(v);
    info.heatmap = heatmap(v, video_id);
    Ok(info)
}

/// Filter chips under `v`, in either shape: `chipCloudChipRenderer` or `chipViewModel`. A chip
/// loads through a continuation on `endpoint`; the selected one has none.
fn chips(v: &Value, endpoint: &str) -> Vec<Chip> {
    let mut found = Vec::new();
    find_all(v, "chipCloudChipRenderer", &mut found);
    let mut models = Vec::new();
    find_all(v, "chipViewModel", &mut models);
    let token = |c: &Value| {
        let mut commands = Vec::new();
        find_all(c, "continuationCommand", &mut commands);
        commands
            .first()
            .and_then(|cmd| cmd["token"].as_str())
            .map(|token| Continuation {
                endpoint: endpoint.to_string(),
                token: token.to_string(),
                owner: None,
            })
    };
    found
        .iter()
        .filter_map(|c| {
            Some(Chip {
                label: text(&c["text"])?,
                selected: c["isSelected"].as_bool().unwrap_or(false),
                next: token(c),
            })
        })
        .chain(models.iter().filter_map(|c| {
            Some(Chip {
                label: c["text"].as_str()?.to_string(),
                selected: c["selected"].as_bool().unwrap_or(false),
                next: token(c),
            })
        }))
        .filter(|c| c.selected || c.next.is_some())
        .collect()
}

pub(crate) fn playlist_panel(
    v: &Value,
    playlist_id: Option<&str>,
    current: Option<&str>,
) -> Option<PlaylistPanel> {
    let renderer = find(v, "playlistPanelRenderer").or_else(|| {
        find(
            &v["contents"]["twoColumnWatchNextResults"]["playlist"],
            "playlist",
        )
    })?;
    let id = renderer["playlistId"].as_str().or(playlist_id)?.to_owned();
    let mut entries = Vec::new();
    find_all(
        &renderer["contents"],
        "playlistPanelVideoRenderer",
        &mut entries,
    );
    let mut items = Vec::new();
    let mut selected = None;
    for entry in entries {
        if let Some(video) = video_renderer(entry) {
            if entry["selected"].as_bool() == Some(true) {
                selected = Some(items.len());
            }
            items.push(video);
        }
    }
    let index = selected
        .or_else(|| current.and_then(|id| items.iter().position(|v| v.id == id)))
        .or_else(|| {
            renderer["localCurrentIndex"]
                .as_u64()
                .or_else(|| renderer["currentIndex"].as_u64())
                .and_then(|index| usize::try_from(index).ok())
                .filter(|i| *i < items.len())
        });
    let next = continuation_token(renderer).map(|token| Continuation {
        endpoint: "next".into(),
        token,
        owner: None,
    });
    Some(PlaylistPanel {
        is_mix: id.starts_with("RD"),
        id,
        title: text(&renderer["title"]).unwrap_or_default(),
        items,
        index,
        next,
    })
}

pub(crate) fn playlist_items(v: &Value) -> Page<Video> {
    let mut entries = Vec::new();
    for key in ["playlistVideoRenderer", "playlistPanelVideoRenderer"] {
        find_all(v, key, &mut entries);
    }
    let items = if entries.is_empty() {
        let mut models = Vec::new();
        find_all(&v["contents"], "lockupViewModel", &mut models);
        if models.is_empty() {
            for key in [
                "onResponseReceivedActions",
                "onResponseReceivedCommands",
                "onResponseReceivedEndpoints",
            ] {
                find_all(&v[key], "lockupViewModel", &mut models);
            }
        }
        models.into_iter().filter_map(lockup).collect()
    } else {
        entries.into_iter().filter_map(video_renderer).collect()
    };
    let next = continuation_token(v).map(|token| Continuation {
        endpoint: "browse".into(),
        token,
        owner: None,
    });
    Page { items, next }
}

fn millis(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0)
        .map(|t| t / 1000.0)
}

fn chapters(v: &Value) -> Vec<Chapter> {
    let mut renderers = Vec::new();
    if let Some(markers) = find(&v["playerOverlays"], "markersMap") {
        find_all(markers, "chapterRenderer", &mut renderers);
    }
    let mut chapters: Vec<_> = renderers
        .into_iter()
        .filter_map(|r| {
            Some(Chapter {
                start: millis(&r["timeRangeStartMillis"])?,
                title: text(&r["title"])?,
                thumbnail: largest_image(&r["thumbnail"]["thumbnails"]),
            })
        })
        .collect();
    if chapters.is_empty() {
        let panels = v["engagementPanels"].as_array().into_iter().flatten();
        let mut renderers = Vec::new();
        for panel in panels {
            let renderer = &panel["engagementPanelSectionListRenderer"];
            if renderer["targetId"]
                .as_str()
                .is_some_and(|id| id.contains("chapters"))
            {
                find_all(
                    &renderer["content"],
                    "macroMarkersListItemRenderer",
                    &mut renderers,
                );
            }
        }
        chapters.extend(renderers.into_iter().filter_map(|r| {
            let start = &r["onTap"]["watchEndpoint"]["startTimeSeconds"];
            Some(Chapter {
                start: start
                    .as_f64()
                    .or_else(|| start.as_str()?.parse().ok())
                    .filter(|t: &f64| t.is_finite() && *t >= 0.0)?,
                title: text(&r["title"])?,
                thumbnail: largest_image(&r["thumbnail"]["thumbnails"]),
            })
        }));
    }
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    chapters.dedup_by(|a, b| a.start == b.start);
    chapters
}

/// Most-replayed samples. Current answers carry them as a `macroMarkersListEntity` of type
/// `MARKER_TYPE_HEATMAP` in `frameworkUpdates`; older ones as `heatMarkerRenderer`s in
/// `playerOverlays`.
fn heatmap(v: &Value, video_id: &str) -> Vec<HeatPoint> {
    let mutations = at(v, &["frameworkUpdates", "entityBatchUpdate", "mutations"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let mut points: Vec<HeatPoint> = mutations
        .filter_map(|m| at(m, &["payload", "macroMarkersListEntity"]))
        .filter(|e| {
            e["externalVideoId"]
                .as_str()
                .is_none_or(|id| id == video_id || video_id.is_empty())
        })
        .filter(|e| e["markersList"]["markerType"].as_str() == Some("MARKER_TYPE_HEATMAP"))
        .flat_map(|e| e["markersList"]["markers"].as_array().into_iter().flatten())
        .filter_map(|m| {
            Some(HeatPoint {
                start: millis(&m["startMillis"])?,
                duration: millis(&m["durationMillis"])?,
                intensity: m["intensityScoreNormalized"]
                    .as_f64()
                    .filter(|i| i.is_finite())?
                    .clamp(0.0, 1.0),
            })
        })
        .collect();
    if points.is_empty() {
        let mut renderers = Vec::new();
        find_all(&v["playerOverlays"], "heatMarkerRenderer", &mut renderers);
        points = renderers
            .into_iter()
            .filter_map(|r| {
                Some(HeatPoint {
                    start: millis(&r["timeRangeStartMillis"])?,
                    duration: millis(&r["markerDurationMillis"])?,
                    intensity: r["heatMarkerIntensityScoreNormalized"]
                        .as_f64()
                        .filter(|i| i.is_finite())?
                        .clamp(0.0, 1.0),
                })
            })
            .collect();
    }
    points.sort_by(|a, b| a.start.total_cmp(&b.start));
    points.dedup_by(|a, b| a.start == b.start);
    points
}

pub(crate) fn parse_comments(v: &Value) -> Result<CommentPage> {
    let mut page = CommentPage::default();
    let endpoints = v
        .get("onResponseReceivedEndpoints")
        .ok_or_else(|| Error::Parse("comments: no endpoints".into()))?;
    // Top-level pages wrap each comment in a thread (which knows its replies); reply pages list
    // the comment view models directly.
    let mut threads = Vec::new();
    find_all(endpoints, "commentThreadRenderer", &mut threads);
    let mut entries: Vec<(&Value, Option<&Value>)> = threads
        .iter()
        .filter_map(|t| Some((at(t, &["commentViewModel", "commentViewModel"])?, Some(*t))))
        .collect();
    if threads.is_empty() {
        let mut models = Vec::new();
        find_all(endpoints, "commentViewModel", &mut models);
        entries = models
            .into_iter()
            .map(|m| (m.get("commentViewModel").unwrap_or(m), None))
            .collect();
    }
    if let Some(header) = find(endpoints, "commentsHeaderRenderer") {
        page.total = str_at(header, &["countText", "runs", "0", "text"]).map(str::to_string);
        let mut menus = Vec::new();
        find_all(header, "sortFilterSubMenuRenderer", &mut menus);
        page.sort = menus
            .first()
            .and_then(|m| m["subMenuItems"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some(CommentSort {
                    label: text(&item["title"])?,
                    selected: item["selected"].as_bool().unwrap_or(false),
                    next: Continuation {
                        endpoint: "next".into(),
                        token: str_at(item, &["serviceEndpoint", "continuationCommand", "token"])?
                            .to_string(),
                        owner: None,
                    },
                })
            })
            .collect();
    }
    // The body's "load more" marker is the last item; the header's sort menu has tokens of its own.
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
            && last.get("continuationItemRenderer").is_some()
            && let Some(token) = continuation_token(last)
        {
            page.next = Some(Continuation {
                endpoint: "next".into(),
                token,
                owner: None,
            });
        }
    }
    let mutations =
        at(v, &["frameworkUpdates", "entityBatchUpdate", "mutations"]).and_then(Value::as_array);
    let payload = |kind: &str, key: &str| {
        mutations.into_iter().flatten().find_map(|m| {
            let p = at(m, &["payload", kind])?;
            (p.get("key").and_then(Value::as_str) == Some(key)).then_some(p)
        })
    };
    for (model, thread) in entries {
        let Some(key) = model.get("commentKey").and_then(Value::as_str) else {
            continue;
        };
        let Some(p) = payload("commentEntityPayload", key) else {
            continue;
        };
        let nonempty = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let hearted = model
            .get("toolbarStateKey")
            .and_then(Value::as_str)
            .and_then(|k| payload("engagementToolbarStateEntityPayload", k))
            .and_then(|t| t.get("heartState"))
            .and_then(Value::as_str)
            == Some("TOOLBAR_HEART_STATE_HEARTED");
        page.items.push(Comment {
            id: str_at(p, &["properties", "commentId"])
                .unwrap_or_default()
                .to_string(),
            author: str_at(p, &["author", "displayName"])
                .unwrap_or_default()
                .to_string(),
            author_channel_id: nonempty(str_at(p, &["author", "channelId"])),
            author_avatar: str_at(p, &["author", "avatarThumbnailUrl"]).map(fix_url),
            text: str_at(p, &["properties", "content", "content"])
                .unwrap_or_default()
                .to_string(),
            published: nonempty(str_at(p, &["properties", "publishedTime"])),
            likes: nonempty(str_at(p, &["toolbar", "likeCountNotliked"])),
            reply_count: nonempty(str_at(p, &["toolbar", "replyCount"])),
            by_creator: at(p, &["author", "isCreator"])
                .and_then(Value::as_bool)
                .unwrap_or(false),
            replies: thread
                .and_then(|t| at(t, &["replies", "commentRepliesRenderer"]))
                .and_then(continuation_token)
                .map(|token| Continuation {
                    endpoint: "next".into(),
                    token,
                    owner: None,
                }),
            pinned: model
                .get("pinnedText")
                .and_then(text)
                .is_some_and(|t| !t.is_empty()),
            hearted,
            author_verified: at(p, &["author", "isVerified"])
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(page)
}

pub(crate) fn parse_channel(v: &Value, channel_id: &str) -> Result<ChannelInfo> {
    let meta = at(v, &["metadata", "channelMetadataRenderer"])
        .ok_or_else(|| Error::Parse("channel: no metadata".into()))?;
    let mut info = ChannelInfo {
        id: str_at(meta, &["externalId"])
            .unwrap_or(channel_id)
            .to_string(),
        name: str_at(meta, &["title"]).unwrap_or_default().to_string(),
        description: str_at(meta, &["description"])
            .unwrap_or_default()
            .to_string(),
        avatar: largest_image(&meta["avatar"]["thumbnails"]).map(|url| resize_avatar(&url, 320)),
        ..Default::default()
    };
    if let Some(header) = at(
        v,
        &[
            "header",
            "pageHeaderRenderer",
            "content",
            "pageHeaderViewModel",
        ],
    ) {
        info.verified = header.get("title").is_some_and(verified_mark);
        if let Some(banner) = header.get("banner") {
            let mut sources = Vec::new();
            find_all(banner, "sources", &mut sources);
            info.banner = sources.first().and_then(|sources| largest_image(sources));
        }
        if let Some(rows) = at(
            header,
            &["metadata", "contentMetadataViewModel", "metadataRows"],
        )
        .and_then(Value::as_array)
        {
            for row in rows {
                for part in row
                    .get("metadataParts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(t) = str_at(part, &["text", "content"]) else {
                        continue;
                    };
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
    let tabs = &v["contents"]["twoColumnBrowseResultsRenderer"]["tabs"];
    for tab in tabs.as_array().into_iter().flatten() {
        let tab = &tab["tabRenderer"];
        let kind = match tab["title"].as_str() {
            Some("Home") => ChannelTab::Home,
            Some("Videos") => ChannelTab::Videos,
            Some("Live") => ChannelTab::Live,
            Some("Playlists") => ChannelTab::Playlists,
            _ => continue,
        };
        info.tabs.push(kind);
        if tab["selected"].as_bool() != Some(true) {
            continue;
        }
        info.selected_tab = Some(kind);
        let content = &tab["content"];
        if kind == ChannelTab::Playlists {
            info.playlists = playlists(content);
        } else {
            info.videos = page(content, "browse");
        }
        if kind == ChannelTab::Videos {
            let header = &content["richGridRenderer"]["header"];
            info.video_sort = chips(header, "browse");
        } else if kind == ChannelTab::Home {
            info.featured = find(content, "channelVideoPlayerRenderer").and_then(|p| {
                let id = p["videoId"].as_str()?;
                Some(Video {
                    id: id.into(),
                    title: text(&p["title"])?,
                    thumbnail: Video::thumbnail_url(id),
                    ..Video::default()
                })
            });
        }
    }
    let owner = Some((info.id.clone(), info.name.clone(), info.avatar.clone()));
    if let Some(next) = &mut info.videos.next {
        next.owner = owner.clone();
    }
    for chip in &mut info.video_sort {
        if let Some(next) = &mut chip.next {
            next.owner = owner.clone();
        }
    }
    for video in info.videos.items.iter_mut().chain(info.featured.iter_mut()) {
        video.channel = info.name.clone();
        video.channel_id = Some(info.id.clone());
        video.channel_avatar = info.avatar.clone();
        video.channel_verified = info.verified;
    }
    let header = &v["header"]["pageHeaderRenderer"]["content"]["pageHeaderViewModel"];
    let attribution = &header["attribution"]["attributionViewModel"]["text"];
    info.links_label = text(attribution).map(|s| s.trim().to_owned());
    info.links = find(attribution, "urlEndpoint")
        .and_then(|e| {
            Some(ChannelLink {
                url: external_url(e["url"].as_str()?)?,
                label: info.links_label.clone()?,
            })
        })
        .into_iter()
        .collect();
    info.about = continuation_token(&header["description"]).map(|token| Continuation {
        endpoint: "browse".into(),
        token,
        owner: None,
    });
    Ok(info)
}

pub(crate) fn playlists(v: &Value) -> Page<PlaylistRef> {
    let mut parsed = Vec::new();
    collect_search(v, &mut parsed);
    let items = parsed
        .into_iter()
        .filter_map(|entry| match entry {
            SearchResult::Playlist(p) => Some(p),
            _ => None,
        })
        .collect();
    let next = continuation_token(v).map(|token| Continuation {
        endpoint: "browse".into(),
        token,
        owner: None,
    });
    Page { items, next }
}

fn external_url(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw).ok()?;
    if url.host_str() == Some("www.youtube.com") && url.path() == "/redirect" {
        let (_, target) = url.query_pairs().find(|(key, _)| key == "q")?;
        url = reqwest::Url::parse(&target).ok()?;
    }
    matches!(url.scheme(), "http" | "https").then(|| url.into())
}

pub(crate) fn channel_about(v: &Value) -> Result<ChannelAbout> {
    let about = find(v, "aboutChannelViewModel")
        .or_else(|| find(v, "channelAboutFullMetadataRenderer"))
        .ok_or_else(|| Error::Parse("channel: missing About panel".into()))?;
    let description = text(&about["description"]).unwrap_or_default();
    let mut renderers = Vec::new();
    find_all(
        &about["links"],
        "channelExternalLinkViewModel",
        &mut renderers,
    );
    let links = renderers
        .into_iter()
        .filter_map(|link| {
            let raw = find(&link["link"], "urlEndpoint")
                .and_then(|e| e["url"].as_str())
                .or_else(|| {
                    find(&link["link"], "webCommandMetadata").and_then(|e| e["url"].as_str())
                })?;
            let url = external_url(raw)?;
            let label = text(&link["title"])
                .or_else(|| text(&link["link"]))
                .unwrap_or_else(|| url.clone());
            Some(ChannelLink { label, url })
        })
        .collect();
    Ok(ChannelAbout { description, links })
}

/// Rewrite a `yt3` avatar URL's size parameter (`=s900-…` to `=s176-…`).
fn resize_avatar(url: &str, size: u32) -> String {
    let Some((base, rest)) = url.rsplit_once("=s") else {
        return url.to_string();
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    match rest.get(digits..).filter(|_| digits > 0) {
        Some(tail) => format!("{base}=s{size}{tail}"),
        None => url.to_string(),
    }
}

/// `window.google.ac.h(["q",[["suggestion",0,[…]],…],…])`
pub(crate) fn parse_suggestions(body: &str) -> Vec<String> {
    let Some(start) = body.find('(') else {
        return Vec::new();
    };
    let Some(end) = body.rfind(')') else {
        return Vec::new();
    };
    let Some(json) = body.get(start + 1..end) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
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
        node.descendants()
            .find(|n| n.tag_name().name() == name)
            .and_then(|n| n.text())
            .map(str::to_string)
    };
    let mut out = Vec::new();
    for entry in doc.descendants().filter(|n| n.tag_name().name() == "entry") {
        let Some(id) = child_text(entry, "videoId") else {
            continue;
        };
        let published = child_text(entry, "published")
            .and_then(|s| parse_rfc3339(&s))
            .unwrap_or(0);
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
                moment: None,
                id,
                ..Default::default()
            },
        ));
    }
    Ok(out)
}

/// "2026-11-10T18:19:55+00:00" to Unix seconds. Offsets other than UTC are honoured.
fn parse_rfc3339(s: &str) -> Option<i64> {
    crate::format::iso_to_unix(s)
        .filter(|(_, timed)| *timed)
        .map(|(t, _)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_header_tabs_and_sort_from_web() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/web-channel-tabs.json")).unwrap();
        let info = parse_channel(&v, "UCSp-OaMpsO8K0KkOqyBl7_w").unwrap();
        assert_eq!(
            info.tabs,
            vec![
                ChannelTab::Home,
                ChannelTab::Videos,
                ChannelTab::Live,
                ChannelTab::Playlists
            ]
        );
        assert_eq!(info.selected_tab, Some(ChannelTab::Videos));
        assert!(
            info.video_sort
                .iter()
                .any(|c| c.label == "Popular" && c.next.is_some())
        );
        assert!(info.about.is_some());
        assert_eq!(info.links[0].url, "https://letsgetrusty.com/join");
        let about: Value =
            serde_json::from_str(include_str!("testdata/web-channel-about.json")).unwrap();
        assert_eq!(
            channel_about(&about).unwrap().links[0].label,
            "Rust Live Accelerator waitlist"
        );
    }

    #[test]
    fn playlist_panel_and_new_browse_lockups() {
        let v: Value = serde_json::from_str(include_str!("testdata/web-playlist.json")).unwrap();
        let panel = playlist_panel(&v["watch"], None, None).unwrap();
        assert_eq!(panel.index, Some(0));
        assert!(panel.previous_video().is_none());
        assert_eq!(panel.next_video().unwrap().id, "H0xBSbnQYds");
        assert_eq!(playlist_items(&v["browse"]).items[0].id, "OX9HJsJUDxA");
    }

    #[test]
    fn search_controls_and_entries_from_web() {
        let raw: Value =
            serde_json::from_str(include_str!("testdata/web-search-mixed.json")).unwrap();
        let page = search_page(&raw, "lets get rusty", false).unwrap();
        assert!(
            page.chips
                .iter()
                .any(|c| c.label == "Unwatched" && matches!(c.filter, Some(SearchFilter::Chip(_))))
        );
        assert!(!page.chips.iter().any(|c| c.label == "Shorts"));
        assert!(page.filters.iter().any(|g| {
            g.title == "Features"
                && g.options
                    .iter()
                    .any(|o| o.label == "4K" && matches!(o.filter, Some(SearchFilter::Params(_))))
        }));
        assert!(page.entries.iter().any(|v| matches!(v, SearchResult::Channel(c) if c.handle.as_deref() == Some("@letsgetrusty") && c.channel.avatar.is_some())));
        assert!(page.entries.iter().any(|v| matches!(v, SearchResult::Playlist(p) if p.video_count.as_deref() == Some("44 videos") && p.thumbnail.is_some())));
        assert!(page.entries.iter().any(|v| matches!(v, SearchResult::Shelf(s) if matches!(s.items.first(), Some(SearchResult::Video(_))))));
        assert!(
            page.notices
                .iter()
                .any(|n| matches!(n, SearchNotice::DidYouMean { .. }))
        );
    }

    #[test]
    fn youtube_search_age_gate_and_corrections() {
        let v: Value = serde_json::from_str(include_str!("testdata/web-age-gate.json")).unwrap();
        assert!(parse_search(&v).unwrap().items.is_empty());
        assert_eq!(
            search_notices(&v, "pronsh", true),
            vec![SearchNotice::AgeGated {
                title: "Confirm your age".into(),
                body: "These results may be inappropriate for some users.".into(),
            }]
        );
        let v = serde_json::json!({"contents":{"twoColumnSearchResultsRenderer":{
            "primaryContents":[{"didYouMeanRenderer":{"correctedQuery":{"runs":[{"text":"rust"}]}}},
                {"showingResultsForRenderer":{"correctedQuery":{"simpleText":"rust programming"}}}]
        }}});
        assert_eq!(
            search_notices(&v, "ruts", false),
            vec![
                SearchNotice::Corrected {
                    showing: "rust programming".into(),
                    original: "ruts".into()
                },
                SearchNotice::DidYouMean {
                    suggestion: "rust".into()
                },
            ]
        );
    }

    #[test]
    fn watch_chapters_and_heatmap() {
        let mut v: Value =
            serde_json::from_str(include_str!("testdata/web-chapters.json")).unwrap();
        let found = chapters(&v);
        assert_eq!(found.len(), 5);
        assert_eq!((found[0].start, found[0].title.as_str()), (0.0, "Intro"));
        assert!(found[0].thumbnail.is_some());
        assert!(heatmap(&v, "").is_empty());
        v["playerOverlays"]["marker"] = serde_json::json!({"heatMarkerRenderer":{
            "timeRangeStartMillis": "2000", "markerDurationMillis": 500,
            "heatMarkerIntensityScoreNormalized": 0.85
        }});
        assert_eq!(
            heatmap(&v, ""),
            vec![HeatPoint {
                start: 2.0,
                duration: 0.5,
                intensity: 0.85
            }]
        );
    }

    #[test]
    fn comment_threads_sort_pinned_hearted_and_replies() {
        let v: Value = serde_json::from_str(include_str!("testdata/web-comments.json")).unwrap();
        let page = parse_comments(&v).unwrap();
        assert_eq!(page.items.len(), 4);
        let labels: Vec<_> = page
            .sort
            .iter()
            .map(|s| (s.label.as_str(), s.selected))
            .collect();
        assert_eq!(labels, vec![("Top", true), ("Newest", false)]);
        assert_ne!(page.sort[0].next.token, page.sort[1].next.token);
        let pinned = &page.items[0];
        assert!(pinned.pinned && pinned.hearted && pinned.by_creator);
        assert_eq!(pinned.reply_count.as_deref(), Some("1"));
        assert!(pinned.replies.is_some());
        assert!(page.items[1..].iter().all(|c| !c.pinned));
        assert!(page.items.iter().filter(|c| c.hearted).count() >= 1);
        assert!(page.next.is_some());
        // A page of replies lists comment view models without threads.
        let v: Value = serde_json::from_str(include_str!("testdata/web-replies.json")).unwrap();
        let replies = parse_comments(&v).unwrap();
        assert_eq!(replies.items.len(), 3);
        assert!(
            replies
                .items
                .iter()
                .all(|c| c.replies.is_none() && !c.text.is_empty())
        );
        assert!(replies.sort.is_empty());
        // "Show more replies" is a button continuation.
        assert!(replies.next.is_some());
    }

    #[test]
    fn card_badges_verified_upcoming_and_chips() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/web-badges-chips.json")).unwrap();
        let card = video_renderer(&v["videoRenderer"]).unwrap();
        assert_eq!(card.badges, vec!["4K", "CC"]);
        assert!(card.channel_verified);
        assert_eq!(card.upcoming, None);
        let lockup = lockup(&v["lockupViewModel"]).unwrap();
        assert_eq!(lockup.badges, vec!["New"]);
        assert!(lockup.channel_verified);
        assert!(!lockup.channel.is_empty());
        let mut plain = v["videoRenderer"].clone();
        plain.as_object_mut().unwrap().remove("ownerBadges");
        plain["upcomingEventData"] = serde_json::json!({
            "startTime": (unix_now() + 3 * 3600 + 60).to_string(),
            "upcomingEventText": {"runs": [{"text": "Premieres "}, {"text": "DATE_PLACEHOLDER"}]}
        });
        let plain = video_renderer(&plain).unwrap();
        assert!(!plain.channel_verified);
        assert_eq!(plain.upcoming.as_deref(), Some("Premieres in 3 hours"));
        // Search header chips: "All" is selected and has no continuation; the rest load one.
        let found = chips(&v["searchHeader"], "search");
        assert_eq!(found[0].label, "All");
        assert!(found[0].selected && found[0].next.is_none());
        assert!(found.len() >= 4);
        assert!(found[1..].iter().all(|c| !c.selected && c.next.is_some()));
        // The same shape under a watch page's related list.
        let watch = serde_json::json!({"contents": {"twoColumnWatchNextResults": {
            "results": {"results": {"contents": [{"videoPrimaryInfoRenderer": {"title": {"simpleText": "T"}}}]}},
            "secondaryResults": {"secondaryResults": {"results": [
                {"relatedChipCloudRenderer": {"content": {"chipCloudRenderer": v["searchHeader"]["searchHeaderRenderer"]["chipBar"]["chipCloudRenderer"].clone()}}},
                {"videoRenderer": v["videoRenderer"].clone()}
            ]}}
        }}});
        let info = parse_watch(&watch, "x").unwrap();
        assert_eq!(info.related_chips.len(), found.len());
        assert_eq!(
            info.related_chips[1].next.as_ref().unwrap().endpoint,
            "next"
        );
        assert_eq!(info.related.items.len(), 1);
    }

    #[test]
    fn heatmap_from_the_framework_entity() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/web-heatmap-entity.json")).unwrap();
        let info = parse_watch(&v, "0y6RKiIk6cs").unwrap();
        assert_eq!(info.heatmap.len(), 100);
        assert_eq!(
            (info.heatmap[0].start, info.heatmap[0].duration),
            (0.0, 7.5)
        );
        assert!((info.heatmap[0].intensity - 0.7103897254846924).abs() < 1e-12);
        assert_eq!(info.heatmap[99].start, 742.5);
        assert!(info.heatmap.windows(2).all(|w| w[0].start < w[1].start));
        assert!(
            info.heatmap
                .iter()
                .all(|p| (0.0..=1.0).contains(&p.intensity))
        );
        // Another video's entity is ignored.
        assert!(parse_watch(&v, "someOtherId").unwrap().heatmap.is_empty());
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-11-10T18:19:55+00:00"),
            Some(1_794_334_795)
        );
        assert_eq!(
            parse_rfc3339("2026-11-10T19:19:55+01:00"),
            Some(1_794_334_795)
        );
    }

    #[test]
    fn avatar_resize() {
        assert_eq!(
            resize_avatar("https://x/abc=s900-c-k-no-rj", 176),
            "https://x/abc=s176-c-k-no-rj"
        );
        assert_eq!(resize_avatar("https://x/abc", 176), "https://x/abc");
    }

    #[test]
    fn suggestions() {
        let body = r#"window.google.ac.h(["rust",[["rust programming",0,[512]],["rust game",0,[]]],{"k":1}])"#;
        assert_eq!(
            parse_suggestions(body),
            vec!["rust programming", "rust game"]
        );
    }

    #[test]
    fn counts() {
        assert!(looks_like_count("6.1B"));
        assert!(looks_like_count("1,234"));
        assert!(!looks_like_count("officialpsy"));
        assert!(!looks_like_count("14y ago"));
    }
}
