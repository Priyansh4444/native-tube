//! Remote images: fetched and decoded off the UI thread, downscaled to the size
//! they're drawn at, deduplicated while in flight, and kept in an LRU bounded by
//! decoded bytes.

use crate::client::{Job, client};
use crate::error::{Error, Result};
use crate::model::RgbaImage;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::OnceCell;

/// Decoded pixels kept across all images. Thumbnails at 320x180 are 230 KB each, so ~400 of them.
const BUDGET_BYTES: usize = 96 * 1024 * 1024;
/// Downloads in flight at once. More just queues behind the same connection pool.
const PARALLEL_FETCHES: usize = 8;

type Slot = Arc<OnceCell<Result<Arc<RgbaImage>>>>;

struct Cache {
    slots: HashMap<(String, u32), Slot>,
    order: VecDeque<(String, u32)>,
    bytes: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Cache {
            slots: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        })
    })
}

/// At most [`PARALLEL_FETCHES`] downloads at once, and when they queue, the **newest** request
/// goes first: after a fast scroll the thumbnails now on screen are the last ones asked for, so
/// they must not wait behind dozens the viewer has already scrolled past.
struct LifoGate {
    state: Mutex<(usize, Vec<tokio::sync::oneshot::Sender<()>>)>,
}

struct GateTurn;

impl LifoGate {
    async fn acquire(&'static self) -> GateTurn {
        let rx = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.0 < PARALLEL_FETCHES {
                st.0 += 1;
                return GateTurn;
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            st.1.push(tx);
            rx
        };
        // The turn is handed over by a finishing fetch; the count stays the same.
        let _ = rx.await;
        GateTurn
    }
}

impl Drop for GateTurn {
    fn drop(&mut self) {
        let gate = fetch_gate();
        let mut st = gate.state.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(next) = st.1.pop() {
            if next.send(()).is_ok() {
                return;
            }
        }
        st.0 = st.0.saturating_sub(1);
    }
}

fn fetch_gate() -> &'static LifoGate {
    static G: OnceLock<LifoGate> = OnceLock::new();
    G.get_or_init(|| LifoGate {
        state: Mutex::new((0, Vec::new())),
    })
}

/// An image decoded to RGBA and downscaled so it is at most `max_width` pixels wide
/// (pass the drawn width in physical pixels; `0` keeps the original size).
pub fn image(url: &str, max_width: u32) -> Job<Arc<RgbaImage>> {
    let key = (url.to_string(), max_width);
    let slot = {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = c.slots.get(&key).cloned() {
            if let Some(pos) = c.order.iter().position(|k| *k == key) {
                let k = c.order.remove(pos).unwrap_or_else(|| key.clone());
                c.order.push_back(k);
            }
            slot
        } else {
            let slot: Slot = Arc::new(OnceCell::new());
            c.slots.insert(key.clone(), slot.clone());
            c.order.push_back(key.clone());
            slot
        }
    };
    if let Some(done) = slot.get() {
        return Job::ready(done.clone());
    }
    let (tx, rx_job) = job_pair();
    crate::client::runtime().spawn(async move {
        let (url, max_width) = key.clone();
        let mine = slot.clone();
        let own_key = key.clone();
        let result = slot
            .get_or_init(|| async move {
                let bytes = {
                    let _turn = fetch_gate().acquire().await;
                    client().get_bytes(&url).await?
                };
                let img = tokio::task::spawn_blocking(move || decode(&bytes, max_width))
                    .await
                    .map_err(|_| Error::Cancelled)?
                    .map(Arc::new)?;
                // Counted exactly once, by the job that decoded it, and only if this slot is
                // still the one in the cache (it can be evicted or replaced while in flight).
                admit(&key, &mine, img.pixels.len());
                Ok(img)
            })
            .await
            .clone();
        if result.is_err() {
            // Failures aren't cached, so a flaky network is retried next time. Remove only our
            // own slot: a newer request for the same image may already sit under the key.
            let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
            if c.slots.get(&own_key).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                c.slots.remove(&own_key);
                c.order.retain(|o| *o != own_key);
            }
        }
        let _ = tx.send(result);
    });
    rx_job
}

fn job_pair<T: Send + 'static>() -> (tokio::sync::oneshot::Sender<Result<T>>, Job<T>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (tx, Job::from_receiver(rx))
}

/// Count a decoded image against the budget, then evict least recently used *finished* images
/// until the cache fits. Images still loading are never evicted: their bytes aren't counted yet.
fn admit(key: &(String, u32), slot: &Slot, len: usize) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if !c.slots.get(key).is_some_and(|s| Arc::ptr_eq(s, slot)) {
        return;
    }
    c.bytes += len;
    let mut i = 0;
    while c.bytes > BUDGET_BYTES && i < c.order.len() {
        let k = c.order[i].clone();
        if k == *key {
            i += 1;
            continue;
        }
        let finished = c.slots.get(&k).and_then(|s| s.get().cloned());
        match finished {
            Some(done) => {
                c.order.remove(i);
                c.slots.remove(&k);
                if let Ok(img) = done {
                    c.bytes = c.bytes.saturating_sub(img.pixels.len());
                }
            }
            None => i += 1, // still loading
        }
    }
}

/// Decoded bytes currently held by the cache.
pub fn cached_bytes() -> usize {
    cache().lock().map(|c| c.bytes).unwrap_or(0)
}

pub fn decode(bytes: &[u8], max_width: u32) -> Result<RgbaImage> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Parse(format!("image: {e}")))?;
    let img = if max_width > 0 && img.width() > max_width {
        let h =
            (u64::from(img.height()) * u64::from(max_width) / u64::from(img.width())).max(1) as u32;
        img.resize_exact(max_width, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    Ok(RgbaImage {
        width: rgba.width(),
        height: rgba.height(),
        pixels: bytes::Bytes::from(rgba.into_raw()),
    })
}

impl RgbaImage {
    /// The same pixels with red and blue swapped, for toolkits that upload BGRA.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut out = self.pixels.to_vec();
        for px in out.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(len: usize) -> Slot {
        let img = RgbaImage {
            width: 1,
            height: 1,
            pixels: bytes::Bytes::from(vec![0u8; len]),
        };
        Arc::new(OnceCell::new_with(Some(Ok(Arc::new(img)))))
    }

    /// One decode is counted once; a slot that was evicted or replaced while loading is not
    /// counted; loading slots are never evicted. (Uses the global cache with private keys.)
    #[test]
    fn accounting() {
        let before = cached_bytes();
        let k = |n: &str| (format!("test://{n}"), 0u32);
        let a = done(1000);
        {
            let mut c = cache().lock().unwrap();
            c.slots.insert(k("a"), a.clone());
            c.order.push_back(k("a"));
        }
        admit(&k("a"), &a, 1000);
        assert_eq!(cached_bytes() - before, 1000);
        // A slot no longer in the cache isn't counted.
        let gone = done(5000);
        admit(&k("gone"), &gone, 5000);
        assert_eq!(cached_bytes() - before, 1000);
        // A replaced slot isn't counted either.
        let other = done(7000);
        admit(&k("a"), &other, 7000);
        assert_eq!(cached_bytes() - before, 1000);
        let mut c = cache().lock().unwrap();
        c.slots.remove(&k("a"));
        c.order.retain(|o| *o != k("a"));
        c.bytes -= 1000;
    }
}
