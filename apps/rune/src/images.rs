//! Thumbnails and avatars. `nt_core::images::image` fetches, decodes and downscales on core's
//! workers; its callback (still on a worker) copies the straight RGBA into a `rune::Pixels`,
//! whose `set` wakes only the window that shows it. Nothing decodes or copies on the UI
//! thread. The view asks for an image only while building a mounted block (on screen or
//! within a viewport of it); entries the view hasn't asked for lately are dropped, which
//! releases Rune's atlas slot once nothing draws them.

use rune::Pixels;
use std::cell::RefCell;
use std::collections::HashMap;

/// Entries kept at most; older unused ones go first.
const KEEP: usize = 96;

struct Entry {
    pixels: Pixels,
    used: u64,
}

#[derive(Default)]
pub struct Images {
    map: RefCell<HashMap<(String, u32), Entry>>,
    epoch: std::cell::Cell<u64>,
}

impl Images {
    /// The image at `url`, at most `width_px` physical pixels wide. Paints nothing until it
    /// arrives (callers draw a raised placeholder under it).
    pub fn get(&self, url: &str, width_px: u32) -> Pixels {
        let epoch = self.epoch.get();
        let mut map = self.map.borrow_mut();
        let key = (url.to_string(), width_px);
        if let Some(e) = map.get_mut(&key) {
            e.used = epoch;
            return e.pixels.clone();
        }
        let pixels = Pixels::new();
        if !url.is_empty() {
            let px = pixels.clone();
            nt_core::images::image(url, width_px).on_done(move |r| {
                if let Ok(img) = r
                    && img.width > 0
                    && img.height > 0
                {
                    px.set(img.width, img.height, img.pixels.to_vec());
                }
            });
        }
        map.insert(key, Entry { pixels: pixels.clone(), used: epoch });
        pixels
    }

    /// Called after each view run: ages entries and trims the ones not used recently.
    pub fn end_view(&self) {
        let epoch = self.epoch.get() + 1;
        self.epoch.set(epoch);
        let mut map = self.map.borrow_mut();
        if map.len() > KEEP {
            let mut ages: Vec<u64> = map.values().map(|e| e.used).collect();
            ages.sort_unstable();
            let cut = ages[map.len() - KEEP];
            // Never drop what the last view used.
            let cut = cut.min(epoch - 1);
            map.retain(|_, e| e.used >= cut);
        }
    }
}
