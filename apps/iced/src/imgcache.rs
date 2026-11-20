//! iced image handles for thumbnails and avatars.
//!
//! `view` asks for an image only for rows it builds (viewport plus one viewport of overscan).
//! A miss starts `nt_core::images::image` (fetch + decode + downscale on core threads); its
//! callback posts the pixels back as a message. The handle wraps the core's `Bytes` without a
//! copy and keeps a stable id.
//!
//! GPU residency: iced's atlas drops every image that was not drawn since its last trim, and the
//! scroller only draws rows that intersect the viewport, so overscan thumbnails would be dropped
//! and re-uploaded synchronously in the frame they scroll in. Each ready image therefore gets an
//! explicit `image::Allocation` (uploaded on iced's worker, exempt from trimming) while it is
//! among the `RESIDENT` most recently viewed; older allocations are released.
//!
//! Bounded in entries (`CAP`), decoded bytes (`BYTES`) and failures (`FAILED_CAP`).

use crate::ext::{self, Ext};
use iced::advanced::image::Allocation;
use iced::widget::image::Handle;
use nt_core::RgbaImage;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

const CAP: usize = 320;
const BYTES: usize = 64 * 1024 * 1024;
const RESIDENT: usize = 160;
const FAILED_CAP: usize = 200;
/// Failed images are asked for again after this long, if still on screen.
const RETRY_SECS: f64 = 5.0;

enum Slot {
    Loading,
    Ready {
        handle: Handle,
        bytes: usize,
        used: Cell<u64>,
        alloc: Option<Allocation>,
    },
    Failed(f64),
}

#[derive(Default)]
pub struct Images {
    by_width: RefCell<HashMap<u32, HashMap<String, Slot>>>,
    view_tick: Cell<u64>,
    ready: usize,
    bytes: usize,
    failed: usize,
}

/// Round widths up so resizing doesn't ask for a new size on every pixel.
pub fn bucket(width_px: f32) -> u32 {
    let w = if width_px.is_finite() {
        width_px.ceil().max(1.0) as u32
    } else {
        1
    };
    w.div_ceil(64).saturating_mul(64)
}

impl Images {
    /// Start of a `view`: everything touched from now on counts as recently used.
    pub fn begin_view(&self) {
        self.view_tick.set(self.view_tick.get().wrapping_add(1));
    }

    /// Reuse the selected card's thumbnail during the transition, without a new request.
    pub fn cached(&self, url: &str, width_px: f32) -> Option<Handle> {
        let map = self.by_width.borrow();
        map.iter()
            .filter_map(|(w, slots)| match slots.get(url) {
                Some(Slot::Ready { handle, used, .. }) => Some((*w, handle, used)),
                _ => None,
            })
            .min_by_key(|(w, _, _)| (*w < bucket(width_px), w.abs_diff(bucket(width_px))))
            .map(|(_, handle, used)| {
                used.set(self.view_tick.get());
                handle.clone()
            })
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
            Some(Slot::Ready { handle, used, .. }) => {
                used.set(self.view_tick.get());
                return Some(handle.clone());
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

    /// Store a decoded image. Returns its handle, to be allocated on the GPU.
    pub fn loaded(
        &mut self,
        url: String,
        w: u32,
        result: nt_core::Result<Arc<RgbaImage>>,
    ) -> Option<Handle> {
        let tick = self.view_tick.get();
        let (slot, handle) = match result {
            Ok(img) => {
                let handle = Handle::from_rgba(img.width, img.height, img.pixels.clone());
                let bytes = img.pixels.len();
                self.ready += 1;
                self.bytes += bytes;
                (
                    Slot::Ready {
                        handle: handle.clone(),
                        bytes,
                        used: Cell::new(tick),
                        alloc: None,
                    },
                    Some(handle),
                )
            }
            Err(e) => {
                eprintln!("nt-iced: image {url}: {e}");
                self.failed += 1;
                (Slot::Failed(crate::clock::now()), None)
            }
        };
        let old = self
            .by_width
            .get_mut()
            .entry(w)
            .or_default()
            .insert(url, slot);
        self.forget(old);
        if self.ready > CAP || self.bytes > BYTES || self.failed > FAILED_CAP {
            self.evict();
        }
        handle
    }

    /// Keep the GPU allocation for a handle, if it is still cached; release the oldest beyond
    /// `RESIDENT`.
    pub fn allocated(&mut self, handle: &Handle, alloc: Allocation) {
        let id = handle.id();
        let map = self.by_width.get_mut();
        let mut resident: Vec<(u64, u32, String)> = Vec::new();
        for (w, inner) in map.iter_mut() {
            for (url, slot) in inner.iter_mut() {
                if let Slot::Ready {
                    handle: h,
                    alloc: a,
                    used,
                    ..
                } = slot
                {
                    if h.id() == id {
                        *a = Some(alloc.clone());
                    }
                    if a.is_some() {
                        resident.push((used.get(), *w, url.clone()));
                    }
                }
            }
        }
        if resident.len() > RESIDENT {
            resident.sort_unstable_by_key(|(t, _, _)| *t);
            let drop_n = resident.len() - RESIDENT;
            for (_, w, url) in resident.into_iter().take(drop_n) {
                if let Some(Slot::Ready { alloc, .. }) =
                    map.get_mut(&w).and_then(|m| m.get_mut(&url))
                {
                    *alloc = None;
                }
            }
        }
    }

    fn forget(&mut self, old: Option<Slot>) {
        match old {
            Some(Slot::Ready { bytes, .. }) => {
                self.ready = self.ready.saturating_sub(1);
                self.bytes = self.bytes.saturating_sub(bytes);
            }
            Some(Slot::Failed(_)) => self.failed = self.failed.saturating_sub(1),
            _ => {}
        }
    }

    /// Drop the least recently viewed quarter of ready images (never one the latest view used)
    /// and every failure, until under all bounds.
    fn evict(&mut self) {
        let now_tick = self.view_tick.get();
        let map = self.by_width.get_mut();
        let mut ticks: Vec<u64> = map
            .values()
            .flat_map(|m| m.values())
            .filter_map(|s| match s {
                Slot::Ready { used, .. } => Some(used.get()),
                _ => None,
            })
            .collect();
        ticks.sort_unstable();
        let cut = ticks
            .get(ticks.len() / 4)
            .copied()
            .unwrap_or(0)
            .min(now_tick.saturating_sub(1));
        let (mut ready, mut bytes, mut failed) = (self.ready, self.bytes, self.failed);
        for inner in map.values_mut() {
            inner.retain(|_, s| match s {
                Slot::Ready { used, bytes: b, .. } if used.get() <= cut => {
                    ready = ready.saturating_sub(1);
                    bytes = bytes.saturating_sub(*b);
                    false
                }
                Slot::Failed(_) => {
                    failed = failed.saturating_sub(1);
                    false
                }
                _ => true,
            });
        }
        map.retain(|_, inner| !inner.is_empty());
        self.ready = ready;
        self.bytes = bytes;
        self.failed = failed;
    }
}
