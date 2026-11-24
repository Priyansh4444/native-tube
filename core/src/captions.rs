use crate::{Error, Result, client, innertube::text};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptionTrack {
    pub language_code: String,
    pub label: String,
    pub auto_generated: bool,
    pub translatable: bool,
    base_url: String,
}

impl CaptionTrack {
    /// The signed timed-text URL for this track, in WebVTT format.
    pub(crate) fn timedtext_url(&self, translate_to: Option<&str>) -> Result<String> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| Error::Parse("Invalid timed-text URL".into()))?;
        if url.scheme() != "https"
            || url.host_str() != Some("www.youtube.com")
            || url.path() != "/api/timedtext"
        {
            return Err(Error::Parse("Unexpected timed-text host or path".into()));
        }
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key != "fmt" && key != "tlang")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let mut query = url.query_pairs_mut();
        query.clear().extend_pairs(pairs).append_pair("fmt", "vtt");
        if let Some(code) = translate_to {
            query.append_pair("tlang", code);
        }
        drop(query);
        Ok(url.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptionLanguage {
    pub code: String,
    pub label: String,
}

pub(crate) fn parse_captions(v: &Value) -> (Vec<CaptionTrack>, Vec<CaptionLanguage>) {
    let list = &v["captions"]["playerCaptionsTracklistRenderer"];
    let tracks = list["captionTracks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|track| {
            let language_code = track["languageCode"].as_str()?.to_owned();
            Some(CaptionTrack {
                label: text(&track["name"]).unwrap_or_else(|| language_code.clone()),
                language_code,
                auto_generated: track["kind"].as_str() == Some("asr"),
                translatable: track["isTranslatable"].as_bool().unwrap_or(false),
                base_url: track["baseUrl"].as_str()?.to_owned(),
            })
        })
        .collect();
    let languages = list["translationLanguages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|lang| {
            Some(CaptionLanguage {
                code: lang["languageCode"].as_str()?.to_owned(),
                label: text(&lang["languageName"])?,
            })
        })
        .collect();
    (tracks, languages)
}

pub(crate) async fn caption_input(url: &str) -> Result<String> {
    let body = client().get_bytes(url).await?;
    let text =
        std::str::from_utf8(&body).map_err(|_| Error::Parse("Timed text is not UTF-8".into()))?;
    if !text.trim_start_matches('\u{feff}').starts_with("WEBVTT") || !text.contains(" --> ") {
        return Err(Error::Parse(format!(
            "Timed text returned no WebVTT cues ({} bytes)",
            body.len()
        )));
    }
    // Feed libmpv the WebVTT via a memory stream so the HTTP request stays in the core's gate.
    Ok(format!("memory://{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visionos_captions_and_translation() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/visionos-captions.json")).unwrap();
        let (tracks, languages) = parse_captions(&v);
        assert_eq!(tracks[0].label, "English (auto-generated)");
        assert!(tracks[0].auto_generated);
        assert_eq!(languages.len(), 156);
        assert!(
            languages
                .iter()
                .any(|l| l.code == "fr" && l.label == "French")
        );
        assert_eq!(
            tracks[0].timedtext_url(Some("fr")).unwrap(),
            "https://www.youtube.com/api/timedtext?v=5C_HPTJg5ek&kind=asr&lang=en&fmt=vtt&tlang=fr"
        );
    }
}
