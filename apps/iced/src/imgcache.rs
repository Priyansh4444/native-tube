//! iced image handles for thumbnails and avatars.
//!
//! `view` asks for an image only for rows it builds (viewport plus one viewport of overscan).
//! A miss starts `nt_core::images::image` (fetch + decode + downscale on core threads); its
//! callback posts the pixels back as a message. The handle wraps the core's `Bytes` without a
//! copy and keeps a stable id, so the renderer uploads it to its atlas once and reuses it while
//! it is drawn (iced trims atlas entries that go undrawn). This map is bounded: past `CAP`
//! ready handles the least recently viewed ones are dropped.

use crate::ext::{self, Ext};
use iced::widget::image::Handle;
use nt_core::RgbaImage;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

const CAP: usize = 320;
/// Failed images are asked for again after this long, if still on screen.
const RETRY_SECS: f64 = 5.0;

enum Slot {
    Loading,
    Ready(Handle, Cell<u64>),
    Failed(f64),
}

#[derive(Default)]
pub struct Images {
    by_width: RefCell<HashMap<u32, HashMap<String, Slot>>>,
    view_tick: Cell<u64>,
    ready: Cell<usize>,
}

/// Round widths up so resizing doesn't ask for a new size on every pixel.
pub fn bucket(width_px: f32) -> u32 {
    let w = width_px.ceil().max(1.0) as u32;
    w.div_ceil(32) * 32
}

impl Images {
    /// Start of a `view`: everything touched from now on counts as recently used.
    pub fn begin_view(&self) {
        self.view_tick.set(self.view_tick.get() + 1);
    }

    /// The handle for `url` drawn `width_px` physical pixels wide, starting a load if needed.
    pub fn get(&self, url: &str, width_px: f32) -> Option<Handle> {
        if url.is_empty() {
            return None;
        }
        let w = bucket(width_px);
        let mut map = self.by_width.borrow_mut();
        let inner = map.entry(w).or_default();
        match inner.get(url) {
            Some(Slot::Ready(h, used)) => {
                used.set(self.view_tick.get());
                return Some(h.clone());
            }
            Some(Slot::Loading) => return None,
            Some(Slot::Failed(at)) if crate::clock::now() - at < RETRY_SECS => return None,
            _ => {}
        }
        inner.insert(url.to_string(), Slot::Loading);
        let key = url.to_string();
        nt_core::images::image(url, w).on_done(move |r| ext::send(Ext::Image(key, w, r)));
        None
    }

    pub fn loaded(&mut self, url: String, w: u32, result: nt_core::Result<Arc<RgbaImage>>) {
        let tick = self.view_tick.get();
        let slot = match result {
            Ok(img) => {
                self.ready.set(self.ready.get() + 1);
                Slot::Ready(Handle::from_rgba(img.width, img.height, img.pixels.clone()), Cell::new(tick))
            }
            Err(_) => Slot::Failed(crate::clock::now()),
        };
        let mut map = self.by_width.borrow_mut();
        let old = map.entry(w).or_default().insert(url, slot);
        if matches!(old, Some(Slot::Ready(..))) {
            self.ready.set(self.ready.get() - 1);
        }
        drop(map);
        if self.ready.get() > CAP {
            self.evict();
        }
    }

    fn evict(&mut self) {
        let mut map = self.by_width.borrow_mut();
        let mut ticks: Vec<u64> = map
            .values()
            .flat_map(|m| m.values())
            .filter_map(|s| if let Slot::Ready(_, t) = s { Some(t.get()) } else { None })
            .collect();
        ticks.sort_unstable();
        // Drop the oldest quarter, but never anything used by the latest view.
        let cut = ticks[ticks.len() / 4].min(self.view_tick.get().saturating_sub(1));
        let mut removed = 0;
        for inner in map.values_mut() {
            inner.retain(|_, s| match s {
                Slot::Ready(_, t) if t.get() <= cut => {
                    removed += 1;
                    false
                }
                _ => true,
            });
        }
        self.ready.set(self.ready.get() - removed);
    }
}
