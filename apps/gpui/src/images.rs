//! Thumbnails and avatars as GPU-ready `RenderImage`s.
//!
//! Pixels come from `nt_core::images` (fetched, decoded and downscaled on the core's workers).
//! The BGRA swizzle and the `RenderImage` are built on GPUI's background executor, so the UI
//! thread only inserts a finished handle. The cache is bounded in entries and in bytes: images
//! not drawn recently are dropped and their atlas tiles freed. Lookups borrow the URL, so the
//! frame path allocates nothing for images already known.

use gpui::{Context, RenderImage, Window};
use std::collections::HashMap;
use std::sync::Arc;

/// Ready images kept: about two screens of a dense grid. Each keeps its pixels on the CPU (GPUI
/// re-uploads from them) plus an atlas tile.
const MAX_READY: usize = 160;
const MAX_BYTES: usize = 48 * 1024 * 1024;
/// Entries of any kind (loading, failed) before forgotten ones are swept.
const MAX_ENTRIES: usize = 600;

enum Slot {
    Loading,
    Ready(Arc<RenderImage>, usize),
    Failed(f64),
}

struct Entry {
    width: u32,
    slot: Slot,
    last_used: u64,
}

#[derive(Default)]
pub struct ImageCache {
    map: HashMap<String, Vec<Entry>>,
    frame: u64,
    ready: usize,
    bytes: usize,
    entries: usize,
}

/// Physical widths are bucketed so a resize doesn't refetch every image for a few pixels.
pub fn bucket(w: f32) -> u32 {
    let w = if w.is_finite() {
        w.clamp(16.0, 4096.0)
    } else {
        320.0
    };
    let w = w.ceil() as u32;
    // 64 px steps up to 512, then 128.
    if w <= 512 {
        w.div_ceil(64) * 64
    } else {
        w.div_ceil(128) * 128
    }
}

pub fn to_render_image(img: &nt_core::RgbaImage) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.to_bgra())?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buf)
    ])))
}

pub type Notify<T> = fn(&mut T, &mut Context<T>);
pub type CacheOf<T> = fn(&mut T) -> &mut ImageCache;

impl ImageCache {
    /// Start of a frame: advance the clock used for eviction.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The image for `url` drawn `width_px` physical pixels wide, requesting it if needed.
    /// `on_ready` is called on the owner when it arrives.
    pub fn get<T: 'static>(
        &mut self,
        url: &str,
        width_px: f32,
        now: f64,
        cx: &mut Context<T>,
        cache: CacheOf<T>,
        on_ready: Notify<T>,
    ) -> Option<Arc<RenderImage>> {
        if url.is_empty() {
            return None;
        }
        let width = bucket(width_px);
        let frame = self.frame;
        if let Some(list) = self.map.get_mut(url)
            && let Some(e) = list.iter_mut().find(|e| e.width == width)
        {
            e.last_used = frame;
            match &e.slot {
                Slot::Ready(img, _) => return Some(img.clone()),
                Slot::Loading => return None,
                // Failures are retried after a while (the core doesn't cache them).
                Slot::Failed(at) if now - *at < 15.0 => return None,
                Slot::Failed(_) => e.slot = Slot::Loading,
            }
        } else {
            self.map.entry(url.to_string()).or_default().push(Entry {
                width,
                slot: Slot::Loading,
                last_used: frame,
            });
            self.entries += 1;
        }
        let job = nt_core::images::image(url, width);
        let key = url.to_string();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let img = match result {
                Ok(rgba) => {
                    cx.background_executor()
                        .spawn(
                            async move { to_render_image(&rgba).map(|i| (i, rgba.pixels.len())) },
                        )
                        .await
                }
                Err(e) => {
                    eprintln!("nt-gpui: image {key}: {e}");
                    None
                }
            };
            let _ = this.update(cx, |owner, cx| {
                let c = cache(owner);
                // Dropped from the cache while loading: nothing to do.
                let Some(e) = c
                    .map
                    .get_mut(&key)
                    .and_then(|l| l.iter_mut().find(|e| e.width == width))
                else {
                    return;
                };
                if !matches!(e.slot, Slot::Loading) {
                    return;
                }
                match img {
                    Some((img, bytes)) => {
                        e.slot = Slot::Ready(img, bytes);
                        c.ready += 1;
                        c.bytes += bytes;
                    }
                    None => e.slot = Slot::Failed(nt_core::metrics::now()),
                }
                on_ready(owner, cx);
            });
        })
        .detach();
        None
    }

    /// End of a frame: if over budget, drop the least recently drawn images and free their tiles.
    pub fn evict(&mut self, window: &mut Window) {
        if self.ready <= MAX_READY && self.bytes <= MAX_BYTES && self.entries <= MAX_ENTRIES {
            return;
        }
        let frame = self.frame;
        let mut ages: Vec<(u64, String, u32)> = Vec::new();
        for (url, list) in &self.map {
            for e in list {
                // Never evict what was drawn in the last two frames.
                if e.last_used + 2 < frame && !matches!(e.slot, Slot::Loading) {
                    ages.push((e.last_used, url.clone(), e.width));
                }
            }
        }
        ages.sort_unstable_by_key(|(t, _, _)| *t);
        for (_, url, width) in ages {
            if self.ready <= MAX_READY * 3 / 4
                && self.bytes <= MAX_BYTES * 3 / 4
                && self.entries <= MAX_ENTRIES * 3 / 4
            {
                break;
            }
            let Some(list) = self.map.get_mut(&url) else {
                continue;
            };
            if let Some(pos) = list.iter().position(|e| e.width == width) {
                let e = list.swap_remove(pos);
                self.entries = self.entries.saturating_sub(1);
                if let Slot::Ready(img, bytes) = e.slot {
                    let _ = window.drop_image(img);
                    self.ready = self.ready.saturating_sub(1);
                    self.bytes = self.bytes.saturating_sub(bytes);
                }
            }
            if list.is_empty() {
                self.map.remove(&url);
            }
        }
    }
}
