//! Thumbnails and avatars. `nt_core::images::image` fetches, decodes and downscales on core's
//! workers; its callback (still on a worker) copies the straight RGBA into a `rune::Pixels`,
//! whose `set` wakes only the window that shows it. Nothing decodes or copies on the UI
//! thread. The view asks for an image only while building a mounted block (on screen or
//! within a viewport of it).
//!
//! Bounds: requested widths are rounded up to 32 physical pixels, so a live resize asks again
//! only every 32 pixels, not every pixel. The map is bounded in entries and in (estimated)
//! bytes; the least recently used entries the last view didn't ask for go first, which
//! releases Rune's atlas slot once nothing draws them. A failed image is asked for again, at
//! most every 15 s, the next time a view needs it.

use rune::Pixels;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// Every entry is a second copy of an image core's decoded cache (96 MiB) already holds, and
// Rune mirrors drawn ones in its atlas: keep only about two screens' worth here. An evicted
// image comes back from core's cache without a request. (400 / 64 MiB filled to ~46 MiB in
// 20 opens and held it, part of the 290 -> 480 MiB PSS plateau over 60 opens.)
const MAX_ENTRIES: usize = 160;
const MAX_BYTES: usize = 24 * 1024 * 1024;
const RETRY_AFTER: Duration = Duration::from_secs(15);

struct Entry {
    pixels: Pixels,
    used: u64,
    bytes: usize,
    failed: Arc<AtomicBool>,
    asked: Instant,
}

#[derive(Default)]
pub struct Images {
    map: RefCell<HashMap<(String, u32), Entry>>,
    epoch: Cell<u64>,
    bytes: Cell<usize>,
}

impl Images {
    /// (entries, estimated bytes, decoded bytes actually held) for `NT_MEM_TRACE`.
    pub fn census(&self) -> (usize, usize, usize) {
        let map = self.map.borrow();
        let held = map
            .values()
            .map(|e| {
                let (w, h) = e.pixels.size();
                w as usize * h as usize * 4
            })
            .sum();
        (map.len(), self.bytes.get(), held)
    }
}

/// Rounds a width in physical pixels up to the next multiple of 32.
pub fn bucket(width_px: u32) -> u32 {
    width_px.div_ceil(32).max(1) * 32
}

fn request(url: &str, width: u32, pixels: &Pixels, failed: &Arc<AtomicBool>) {
    let (px, failed) = (pixels.clone(), failed.clone());
    nt_core::images::image(url, width).on_done(move |r| match r {
        Ok(img) if img.width > 0 && img.height > 0 => {
            px.set(img.width, img.height, img.pixels.to_vec());
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("nt-rune: image failed: {e}");
            failed.store(true, Ordering::Release);
        }
    });
}

impl Images {
    /// The image at `url`, drawn `width_px` physical pixels wide. Paints nothing until it
    /// arrives (callers draw a placeholder under it).
    pub fn get(&self, url: &str, width_px: u32) -> Pixels {
        let width = bucket(width_px);
        let epoch = self.epoch.get();
        let mut map = self.map.borrow_mut();
        let key = (url.to_string(), width);
        if let Some(e) = map.get_mut(&key) {
            e.used = epoch;
            if e.failed.load(Ordering::Acquire) && e.asked.elapsed() >= RETRY_AFTER {
                e.failed.store(false, Ordering::Release);
                e.asked = Instant::now();
                request(url, width, &e.pixels, &e.failed);
            }
            return e.pixels.clone();
        }
        let pixels = Pixels::new();
        let failed = Arc::new(AtomicBool::new(false));
        if !url.is_empty() {
            request(url, width, &pixels, &failed);
        }
        // Thumbnails are 16:9; avatars are square but small. Estimate before the size is known.
        let bytes = (width as usize).pow(2) * 9 / 16 * 4;
        self.bytes.set(self.bytes.get() + bytes);
        map.insert(
            key,
            Entry {
                pixels: pixels.clone(),
                used: epoch,
                bytes,
                failed,
                asked: Instant::now(),
            },
        );
        pixels
    }

    /// Called after each view run: ages entries and trims the least recently used ones the last
    /// view didn't ask for, until the map is under its entry and byte bounds.
    pub fn end_view(&self) {
        let epoch = self.epoch.get() + 1;
        self.epoch.set(epoch);
        let mut map = self.map.borrow_mut();
        if map.len() <= MAX_ENTRIES && self.bytes.get() <= MAX_BYTES {
            return;
        }
        let mut old: Vec<(u64, (String, u32))> = map
            .iter()
            .filter(|(_, e)| e.used + 1 < epoch)
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        old.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in old {
            if map.len() <= MAX_ENTRIES && self.bytes.get() <= MAX_BYTES {
                break;
            }
            if let Some(e) = map.remove(&key) {
                self.bytes.set(self.bytes.get().saturating_sub(e.bytes));
            }
        }
    }
}
