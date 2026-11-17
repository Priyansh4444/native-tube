//! Thumbnails and avatars as GPU-ready `RenderImage`s.
//!
//! Pixels come from `nt_core::images` (fetched, decoded and downscaled on the core's
//! workers). The BGRA swizzle and the `RenderImage` are built on GPUI's background
//! executor, so the UI thread only inserts a finished handle. The cache is bounded:
//! entries that haven't been drawn recently are dropped and their atlas tiles freed.

use gpui::{Context, RenderImage, Window};
use std::collections::HashMap;
use std::sync::Arc;

/// Ready images kept around: the ~30 within a viewport of the screen plus a few screens of
/// recent ones. Each keeps its pixels on the CPU (GPUI re-uploads from them) and an atlas tile.
const MAX_READY: usize = 140;

enum Slot {
    Loading,
    Ready(Arc<RenderImage>),
    Failed(f64),
}

struct Entry {
    slot: Slot,
    last_used: u64,
}

#[derive(Default)]
pub struct ImageCache {
    map: HashMap<(String, u32), Entry>,
    frame: u64,
    ready: usize,
}

/// Physical widths are bucketed so a window resize doesn't refetch every image at a 1 px
/// different size (the core caches per exact width).
pub fn bucket(w: f32) -> u32 {
    let w = w.max(16.0).ceil() as u32;
    w.div_ceil(32) * 32
}

pub fn to_render_image(img: &nt_core::RgbaImage) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.to_bgra())?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buf)
    ])))
}

impl ImageCache {
    /// Start of a frame: advance the clock used for eviction.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The image for `url` drawn `width_px` physical pixels wide, requesting it if needed.
    pub fn get<T: 'static>(
        &mut self,
        url: &str,
        width_px: f32,
        now: f64,
        cx: &mut Context<T>,
        on_ready: fn(&mut T) -> &mut ImageCache,
    ) -> Option<Arc<RenderImage>> {
        if url.is_empty() {
            return None;
        }
        let key = (url.to_string(), bucket(width_px));
        let frame = self.frame;
        if let Some(e) = self.map.get_mut(&key) {
            e.last_used = frame;
            match &e.slot {
                Slot::Ready(img) => return Some(img.clone()),
                Slot::Loading => return None,
                // Failures are retried after a while (the core doesn't cache them).
                Slot::Failed(at) if now - *at < 10.0 => return None,
                Slot::Failed(_) => {}
            }
        }
        self.map.insert(
            key.clone(),
            Entry {
                slot: Slot::Loading,
                last_used: frame,
            },
        );
        let job = nt_core::images::image(&key.0, key.1);
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let img = match result {
                Ok(rgba) => {
                    cx.background_executor()
                        .spawn(async move { to_render_image(&rgba) })
                        .await
                }
                Err(_) => None,
            };
            let _ = this.update(cx, |app, cx| {
                let cache = on_ready(app);
                if let Some(e) = cache.map.get_mut(&key) {
                    match img {
                        Some(img) => {
                            e.slot = Slot::Ready(img);
                            cache.ready += 1;
                        }
                        None => e.slot = Slot::Failed(nt_core::metrics::now()),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }

    /// End of a frame: if over budget, drop the least recently drawn images and free their tiles.
    pub fn evict(&mut self, window: &mut Window) {
        if self.ready <= MAX_READY {
            return;
        }
        let mut ages: Vec<(u64, (String, u32))> = self
            .map
            .iter()
            .filter(|(_, e)| matches!(e.slot, Slot::Ready(_)) && e.last_used + 2 < self.frame)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        ages.sort_by_key(|(t, _)| *t);
        let excess = self.ready - MAX_READY * 3 / 4;
        for (_, key) in ages.into_iter().take(excess) {
            if let Some(Entry {
                slot: Slot::Ready(img),
                ..
            }) = self.map.remove(&key)
            {
                let _ = window.drop_image(img);
                self.ready -= 1;
            }
        }
        // Forget stale failures and abandoned loads too, so the map stays small.
        let frame = self.frame;
        self.map
            .retain(|_, e| !matches!(e.slot, Slot::Failed(_)) || e.last_used + 600 > frame);
    }
}
