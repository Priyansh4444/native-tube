//! Seek-bar previews from YouTube's storyboards: sprite sheets of small frames, described by
//! `storyboards.playerStoryboardSpecRenderer.spec` in the `/player` answer.
//!
//! The spec is `template|level|level|…`; each level is
//! `width#height#count#columns#rows#interval_ms#name#signature`. A sheet's URL is the template
//! with `$L` replaced by the level index and `$N` by the name (whose `$M` is the sheet number),
//! plus `&sigh=<signature>`. Sheets load through [`crate::images::image`], so they are cached,
//! deduplicated, fetched newest-request-first, and (being on `/sb/`) wait their turn at the
//! polite gate.

use crate::client::Job;
use crate::error::{Error, Result};
use crate::model::RgbaImage;
use serde_json::Value;
use std::sync::Arc;

/// Every preview size a video offers, smallest first. Only the parser builds one, and it
/// validates every level, so the methods never panic.
#[derive(Clone, Debug, PartialEq)]
pub struct Storyboard {
    levels: Vec<StoryboardLevel>,
    /// A copy of the largest level, so lookups need no fallible indexing.
    largest: StoryboardLevel,
}

/// One preview size. Read it through the methods; the parser guarantees non-zero sizes whose
/// sheet fits in 16 384 px a side.
#[derive(Clone, Debug, PartialEq)]
pub struct StoryboardLevel {
    width: u32,
    height: u32,
    count: u32,
    columns: u32,
    rows: u32,
    interval: f64,
    index: usize,
    template: String,
    name: String,
    signature: String,
}

/// Where one preview frame sits: a sheet URL and a pixel rectangle on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryboardFrame {
    pub sheet_url: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The largest sprite sheet side accepted, in pixels. YouTube's are at most a few thousand.
const MAX_SHEET_PX: u32 = 16_384;

pub(crate) fn parse_storyboard(player: &Value) -> Option<Storyboard> {
    let spec = player
        .pointer("/storyboards/playerStoryboardSpecRenderer/spec")?
        .as_str()?;
    let duration = player
        .pointer("/videoDetails/lengthSeconds")
        .and_then(|v| v.as_str()?.parse::<f64>().ok().or_else(|| v.as_f64()))
        .filter(|d| d.is_finite() && *d > 0.0)?;
    let mut parts = spec.split('|');
    let template = parts.next()?.to_string();
    if !template.starts_with("https://i.ytimg.com/sb/") {
        return None;
    }
    let mut levels: Vec<StoryboardLevel> = parts
        .enumerate()
        .filter_map(|(index, level)| {
            let f: Vec<&str> = level.split('#').collect();
            let [w, h, count, cols, rows, interval, name, signature] = f.as_slice() else {
                return None;
            };
            let num = |s: &str| s.parse::<u32>().ok().filter(|n| *n > 0);
            let (width, height, count) = (num(w)?, num(h)?, num(count)?);
            let (columns, rows) = (num(cols)?, num(rows)?);
            // Every product frame_at forms must fit, and a sheet must be a sane image size.
            columns.checked_mul(rows)?;
            columns.checked_mul(width).filter(|w| *w <= MAX_SHEET_PX)?;
            rows.checked_mul(height).filter(|h| *h <= MAX_SHEET_PX)?;
            let interval_ms = interval.parse::<f64>().ok().filter(|i| i.is_finite())?;
            let interval = if interval_ms > 0.0 {
                interval_ms / 1000.0
            } else {
                duration / f64::from(count)
            };
            if !(interval.is_finite() && interval > 0.0) {
                return None;
            }
            Some(StoryboardLevel {
                width,
                height,
                count,
                columns,
                rows,
                interval,
                index,
                template: template.clone(),
                name: name.to_string(),
                signature: signature.to_string(),
            })
        })
        .collect();
    levels.sort_by_key(|l| l.width);
    let largest = levels.last()?.clone();
    Some(Storyboard { levels, largest })
}

impl StoryboardLevel {
    /// One frame's size in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Frames in the whole level.
    pub fn count(&self) -> u32 {
        self.count
    }
    /// Frames per sheet across and down.
    pub fn columns(&self) -> u32 {
        self.columns
    }
    pub fn rows(&self) -> u32 {
        self.rows
    }
    /// Seconds between frames (always finite and positive).
    pub fn interval(&self) -> f64 {
        self.interval
    }

    /// The frame shown at `seconds`, clamped to the level's range. Any `seconds` is accepted;
    /// NaN and negative values read as 0.
    pub fn frame_at(&self, seconds: f64) -> StoryboardFrame {
        let seconds = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        // `as` saturates, and the parser guarantees count >= 1 and that these products fit.
        let last = u64::from(self.count.saturating_sub(1));
        let index = ((seconds / self.interval).floor() as u64).min(last) as u32;
        let per_sheet = self.columns.saturating_mul(self.rows).max(1);
        let (sheet, cell) = (index / per_sheet, index % per_sheet);
        let name = self.name.replace("$M", &sheet.to_string());
        let mut sheet_url = self
            .template
            .replace("$L", &self.index.to_string())
            .replace("$N", &name);
        if !self.signature.is_empty() {
            sheet_url.push_str(if sheet_url.contains('?') { "&" } else { "?" });
            sheet_url.push_str("sigh=");
            sheet_url.push_str(&self.signature);
        }
        StoryboardFrame {
            sheet_url,
            x: (cell % self.columns) * self.width,
            y: (cell / self.columns) * self.height,
            width: self.width,
            height: self.height,
        }
    }
}

impl Storyboard {
    /// The levels, smallest first; never empty.
    pub fn levels(&self) -> &[StoryboardLevel] {
        &self.levels
    }

    /// The smallest level at least `width_px` wide, or the largest one.
    pub fn level_for_width(&self, width_px: u32) -> &StoryboardLevel {
        self.levels
            .iter()
            .find(|l| l.width >= width_px)
            .unwrap_or(&self.largest)
    }

    /// The preview picture for `seconds`, from the level that suits a drawn width of `width_px`
    /// physical pixels. The result is the frame's own size (e.g. 160x90); scale it when drawing.
    /// Call from an event handler; the sheet loads on core workers.
    pub fn preview(&self, seconds: f64, width_px: u32) -> Job<Arc<RgbaImage>> {
        let frame = self.level_for_width(width_px).frame_at(seconds);
        let sheet = crate::images::image(&frame.sheet_url, 0);
        Job::spawn(async move { crop(sheet.await?.as_ref(), &frame).map(Arc::new) })
    }
}

fn crop(sheet: &RgbaImage, f: &StoryboardFrame) -> Result<RgbaImage> {
    // The last sheet of a level can be shorter than a full grid.
    if f.x + f.width > sheet.width || f.y + f.height > sheet.height {
        return Err(Error::Parse(format!(
            "storyboard frame {}x{}+{}+{} outside the {}x{} sheet",
            f.width, f.height, f.x, f.y, sheet.width, sheet.height
        )));
    }
    let row = (sheet.width * 4) as usize;
    let mut pixels = Vec::with_capacity((f.width * f.height * 4) as usize);
    for y in f.y..f.y + f.height {
        let start = y as usize * row + f.x as usize * 4;
        pixels.extend_from_slice(&sheet.pixels[start..start + f.width as usize * 4]);
    }
    Ok(RgbaImage {
        width: f.width,
        height: f.height,
        pixels: pixels.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visionos_storyboard_levels_and_frames() {
        let v: Value =
            serde_json::from_str(include_str!("testdata/visionos-storyboard.json")).unwrap();
        let sb = parse_storyboard(&v).unwrap();
        let sizes: Vec<_> = sb
            .levels()
            .iter()
            .map(|l| (l.width(), l.height()))
            .collect();
        assert_eq!(sizes, vec![(48, 27), (80, 45), (160, 90)]);
        // Level 0 has no interval: 100 frames over the 149 s video.
        assert!((sb.levels()[0].interval() - 1.49).abs() < 1e-9);
        assert_eq!(
            sb.levels()[0].frame_at(10.0),
            StoryboardFrame {
                sheet_url: "https://i.ytimg.com/sb/5C_HPTJg5ek/storyboard3_L0/default.jpg?sqp=fixture&sigh=rs$AOn4CLDW2CkAuTDirJ5hyEzoVPGRbfsZaw".into(),
                x: 6 * 48,
                y: 0,
                width: 48,
                height: 27,
            }
        );
        let big = sb.level_for_width(120);
        assert_eq!(big.width(), 160);
        // 2 s a frame, 25 a sheet: 61 s is frame 30, sheet 1, cell 5 (row 1, column 0).
        let f = big.frame_at(61.0);
        assert!(
            f.sheet_url
                .contains("storyboard3_L2/M1.jpg?sqp=fixture&sigh=rs$")
        );
        assert_eq!((f.x, f.y), (0, 90));
        // Past the end clamps to the last frame (75: sheet 3, cell 0).
        let last = big.frame_at(1e9);
        assert!(last.sheet_url.contains("/M3.jpg"));
        assert_eq!((last.x, last.y), (0, 0));
        assert_eq!(sb.level_for_width(4000).width(), 160);
        assert_eq!(sb.level_for_width(0).width(), 48);
    }

    #[test]
    fn crop_copies_the_cell() {
        let sheet = RgbaImage {
            width: 4,
            height: 2,
            pixels: (0..32u8).collect::<Vec<_>>().into(),
        };
        let f = StoryboardFrame {
            sheet_url: String::new(),
            x: 2,
            y: 1,
            width: 2,
            height: 1,
        };
        assert_eq!(
            &crop(&sheet, &f).unwrap().pixels[..],
            &[24, 25, 26, 27, 28, 29, 30, 31]
        );
        let outside = StoryboardFrame { y: 2, ..f };
        assert!(crop(&sheet, &outside).is_err());
    }

    #[test]
    fn missing_or_foreign_spec_is_none() {
        assert!(parse_storyboard(&serde_json::json!({})).is_none());
        let foreign = serde_json::json!({"storyboards":{"playerStoryboardSpecRenderer":{
            "spec":"https://example.com/sb/$L/$N.jpg|48#27#100#10#10#0#default#x"}},
            "videoDetails":{"lengthSeconds":"10"}});
        assert!(parse_storyboard(&foreign).is_none());
    }

    proptest::proptest! {
        /// Any spec the parser accepts gives levels whose frames lie inside their sheet, for
        /// any time at all, without overflow (tests build with overflow checks on).
        #[test]
        fn frame_at_never_panics(
            dims in proptest::collection::vec(
                (proptest::prelude::any::<u32>(), proptest::prelude::any::<u32>(),
                 proptest::prelude::any::<u32>(), proptest::prelude::any::<u32>(),
                 proptest::prelude::any::<u32>(), proptest::prelude::any::<f64>()),
                1..4,
            ),
            small in proptest::collection::vec((1u32..300, 1u32..200, 1u32..500, 1u32..20, 1u32..20, -5.0f64..5000.0), 1..4),
            duration in proptest::prelude::any::<f64>(),
            seconds in proptest::collection::vec(proptest::prelude::any::<f64>(), 1..8),
        ) {
            let levels: Vec<String> = dims.iter().chain(small.iter())
                .map(|(w, h, n, c, r, i)| format!("{w}#{h}#{n}#{c}#{r}#{i}#M$M#rs$x"))
                .collect();
            let v = serde_json::json!({
                "storyboards": {"playerStoryboardSpecRenderer": {
                    "spec": format!("https://i.ytimg.com/sb/x/storyboard3_L$L/$N.jpg?sqp=y|{}", levels.join("|"))
                }},
                "videoDetails": {"lengthSeconds": duration.to_string()}
            });
            if let Some(sb) = parse_storyboard(&v) {
                for level in sb.levels() {
                    for &t in &seconds {
                        let f = level.frame_at(t);
                        proptest::prop_assert!(f.x + f.width <= level.columns() * level.width());
                        proptest::prop_assert!(f.y + f.height <= level.rows() * level.height());
                    }
                }
                for &t in &seconds {
                    let _ = sb.level_for_width(t.abs().min(1e6) as u32).frame_at(t);
                }
            }
        }
    }
}
