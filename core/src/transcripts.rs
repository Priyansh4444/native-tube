use crate::{Continuation, Moment, Page, Result, Video};
use serde::Deserialize;

#[derive(Deserialize)]
struct Answer {
    videos: Vec<TranscriptVideo>,
    has_next_page: bool,
    total_videos: u64,
}

#[derive(Deserialize)]
struct TranscriptVideo {
    video_id: String,
    title: String,
    youtuber_name: String,
    youtuber_handle: String,
    thumbnail: Option<String>,
    published_at: Option<String>,
    view_count: u64,
    best_timestamp: Option<String>,
    youtube_url: String,
    #[serde(default)]
    content_kind: String,
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    t_start_ms: u64,
    text: String,
    text_highlight: Option<String>,
    timestamp: Option<String>,
}

pub(crate) fn parse(bytes: &[u8], query: &str, index: u32) -> Result<(Page<Video>, u64)> {
    let found: Answer = serde_json::from_slice(bytes)?;
    let items = found
        .videos
        .into_iter()
        .filter(|v| v.content_kind != "short")
        .map(TranscriptVideo::into_card)
        .collect();
    let next = found
        .has_next_page
        .then(|| index.checked_add(1))
        .flatten()
        .map(|page| {
            serde_json::to_string(&(query, page)).map(|token| Continuation {
                endpoint: "transcript-search".into(),
                token,
                owner: None,
            })
        })
        .transpose()?;
    Ok((Page { items, next }, found.total_videos))
}

impl TranscriptVideo {
    fn into_card(self) -> Video {
        let target = reqwest::Url::parse(&self.youtube_url).ok().and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "t")
                .and_then(|(_, t)| {
                    t.trim_end_matches('s')
                        .parse::<f64>()
                        .ok()
                        .filter(|s| s.is_finite() && *s >= 0.0)
                })
        });
        let matching_hit = self
            .hits
            .iter()
            .find(|hit| hit.timestamp.is_some() && hit.timestamp == self.best_timestamp);
        let hit = matching_hit.or_else(|| self.hits.first());
        let moment = hit.map(|hit| {
            let start_secs = target
                .or_else(|| matching_hit.map(|h| h.t_start_ms as f64 / 1000.0))
                .unwrap_or(hit.t_start_ms as f64 / 1000.0);
            let (snippet, highlights) = hit
                .text_highlight
                .as_deref()
                .map(highlight_ranges)
                .unwrap_or_else(|| (hit.text.clone(), Vec::new()));
            Moment {
                start_secs,
                label: self
                    .best_timestamp
                    .or_else(|| hit.timestamp.clone())
                    .unwrap_or_else(|| crate::format::clock(start_secs)),
                snippet,
                highlights,
            }
        });
        let thumbnail = self
            .thumbnail
            .unwrap_or_else(|| Video::thumbnail_url(&self.video_id));
        Video {
            id: self.video_id,
            title: self.title,
            channel: if self.youtuber_name.is_empty() {
                self.youtuber_handle
            } else {
                self.youtuber_name
            },
            thumbnail,
            views: Some(format!(
                "{} views",
                crate::format::short_count(self.view_count)
            )),
            published: self
                .published_at
                .map(|at| crate::format::published_label(&at, crate::format::unix_now())),
            snippet: moment.as_ref().map(|m| m.snippet.clone()),
            moment,
            ..Video::default()
        }
    }
}

fn highlight_ranges(input: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut plain = String::with_capacity(input.len());
    let mut highlights = Vec::new();
    let mut rest = input;
    while let Some((before, marked)) = rest.split_once("<mark>") {
        let Some((matched, after)) = marked.split_once("</mark>") else {
            break;
        };
        plain.push_str(before);
        let start = plain.len();
        plain.push_str(matched);
        if plain.len() > start {
            highlights.push(start..plain.len());
        }
        rest = after;
    }
    plain.push_str(rest);
    (plain, highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_transcript_match_uses_the_video_timestamp() {
        let (page, total) = parse(
            include_bytes!("testdata/transcript-search.json"),
            "pronsh",
            1,
        )
        .unwrap();
        assert_eq!(total, 7);
        assert_eq!(page.items[0].id, "BzRUzF-LjRk");
        // ISO dates become YouTube's relative wording at parse time.
        for video in &page.items {
            let published = video.published.as_deref().unwrap_or("today");
            assert!(
                published == "today" || published.ends_with(" ago"),
                "{published:?}"
            );
        }
        let moment = page.items[0].moment.as_ref().unwrap();
        assert_eq!(moment.label, "9:52");
        assert_eq!(moment.start_secs.floor(), 592.0);
        for range in &moment.highlights {
            assert!(
                moment
                    .snippet
                    .get(range.clone())
                    .is_some_and(|s| !s.is_empty())
            );
        }
    }

    #[test]
    fn highlights_use_utf8_byte_ranges() {
        let (snippet, ranges) = highlight_ranges("é <mark>資料</mark> and <mark>🦀</mark>");
        assert_eq!(snippet, "é 資料 and 🦀");
        assert_eq!(snippet.get(ranges[0].clone()), Some("資料"));
        assert_eq!(snippet.get(ranges[1].clone()), Some("🦀"));
    }
}
