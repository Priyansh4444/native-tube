//! Remote images: fetched and decoded off the UI thread, downscaled to the size
//! they're drawn at, deduplicated while in flight, and kept in an LRU bounded by
//! decoded bytes.

use crate::client::{Job, client};
use crate::error::{Error, Result};
use crate::model::RgbaImage;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{OnceCell, Semaphore};

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

fn fetch_gate() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(PARALLEL_FETCHES))
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
    let (url, max_width) = key.clone();
    let (tx, rx_job) = job_pair();
    crate::client::runtime().spawn(async move {
        let result = slot
            .get_or_init(|| async move {
                let bytes = {
                    let _permit = fetch_gate().acquire().await.map_err(|_| Error::Cancelled)?;
                    client().get_bytes(&url).await?
                };
                tokio::task::spawn_blocking(move || decode(&bytes, max_width))
                    .await
                    .map_err(|_| Error::Cancelled)?
                    .map(Arc::new)
            })
            .await
            .clone();
        account(&key, &result);
        let _ = tx.send(result);
    });
    rx_job
}

fn job_pair<T: Send + 'static>() -> (tokio::sync::oneshot::Sender<Result<T>>, Job<T>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (tx, Job::from_receiver(rx))
}

/// Count a finished image against the budget, evicting the least recently used. Failures aren't
/// cached, so a flaky network gets retried next time the image is asked for.
fn account(key: &(String, u32), result: &Result<Arc<RgbaImage>>) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(img) => {
            c.bytes += img.pixels.len();
            while c.bytes > BUDGET_BYTES && c.order.len() > 1 {
                let Some(old) = c.order.pop_front() else {
                    break;
                };
                if let Some(slot) = c.slots.remove(&old)
                    && let Some(Ok(img)) = slot.get()
                {
                    c.bytes = c.bytes.saturating_sub(img.pixels.len());
                }
            }
        }
        Err(_) => {
            c.slots.remove(key);
            c.order.retain(|k| k != key);
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
