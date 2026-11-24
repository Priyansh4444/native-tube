//! Remote images: fetched and decoded off the UI thread, downscaled to the size
//! they're drawn at, deduplicated while in flight, and kept in an LRU bounded by
//! decoded bytes.

use crate::client::{Job, client};
use crate::error::{Error, Result};
use crate::model::RgbaImage;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

/// Decoded pixels kept across all images. Thumbnails at 320x180 are 230 KB each, so ~400 of them.
const BUDGET_BYTES: usize = 96 * 1024 * 1024;
/// Downloads in flight at once. More just queues behind the same connection pool.
const PARALLEL_FETCHES: usize = 8;

type Slot = Arc<OnceCell<Result<Arc<RgbaImage>>>>;

struct Entry {
    slot: Slot,
    bytes: Option<usize>,
}

struct Cache {
    slots: HashMap<(String, u32), Entry>,
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
        if let Some(slot) = c.slots.get(&key).map(|e| e.slot.clone()) {
            if let Some(pos) = c.order.iter().position(|k| *k == key) {
                let k = c.order.remove(pos).unwrap_or_else(|| key.clone());
                c.order.push_back(k);
            }
            slot
        } else {
            let slot: Slot = Arc::new(OnceCell::new());
            c.slots.insert(
                key.clone(),
                Entry {
                    slot: slot.clone(),
                    bytes: None,
                },
            );
            c.order.push_back(key.clone());
            slot
        }
    };
    if let Some(done) = slot.get() {
        return Job::ready(done.clone());
    }
    let rt = match crate::client::try_runtime() {
        Ok(rt) => rt,
        Err(e) => return Job::ready(Err(e)),
    };
    let (tx, rx_job) = job_pair();
    rt.spawn(async move {
        let (url, max_width) = key.clone();
        let result = slot
            .get_or_init(|| async {
                let _turn = fetch_gate().acquire().await;
                fetch_image(&url, max_width).await.map(Arc::new)
            })
            .await
            .clone();
        match &result {
            Ok(img) => admit(&key, &slot, img.pixels.len()),
            Err(_) => {
                // A newer request may already have replaced the failed slot.
                let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
                if c.slots
                    .get(&key)
                    .is_some_and(|e| Arc::ptr_eq(&e.slot, &slot))
                {
                    c.slots.remove(&key);
                    c.order.retain(|o| *o != key);
                }
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

/// Account for completed entries exactly once, evicting oldest accounted entries first.
fn admit(key: &(String, u32), slot: &Slot, len: usize) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = c.slots.get_mut(key) else {
        return;
    };
    if !Arc::ptr_eq(&entry.slot, slot) || entry.bytes.is_some() {
        return;
    }
    entry.bytes = Some(len);
    c.bytes += len;
    while c.bytes > BUDGET_BYTES {
        let Some(index) = c
            .order
            .iter()
            .position(|k| c.slots.get(k).is_some_and(|e| e.bytes.is_some()))
        else {
            break;
        };
        if let Some(removed) = c.order.remove(index).and_then(|k| c.slots.remove(&k)) {
            c.bytes -= removed.bytes.unwrap_or_default();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Poster {
    Renderer,
    Hd,
    Sd,
    Hq,
    Medium,
}

/// The widest drawn poster (physical px) that hqdefault's 480 px serves without upscaling.
const SMALL_POSTER_MAX: u32 = 480;

fn poster_candidates(url: &str, max_width: u32) -> Option<(String, Vec<(Poster, String)>)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.host_str()? != "i.ytimg.com" {
        return None;
    }
    let [directory, id, file] = parsed
        .path_segments()?
        .collect::<Vec<_>>()
        .try_into()
        .ok()?;
    if !matches!(directory, "vi" | "vi_webp")
        || id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    let (name, _) = file.split_once('.')?;
    if !matches!(
        name,
        "hq720" | "maxresdefault" | "sddefault" | "hqdefault" | "mqdefault" | "default"
    ) {
        return None; // Animated previews, chapter images and sprite sheets are not posters.
    }
    let canonical = |name: &str| format!("https://i.ytimg.com/vi/{id}/{name}.jpg");
    let mut candidates = Vec::with_capacity(5);
    let renderer = (matches!(name, "hq720" | "maxresdefault") && url != canonical("hq720"))
        .then(|| (Poster::Renderer, url.to_owned()));
    if max_width > 320 && max_width <= SMALL_POSTER_MAX {
        // A card this size needs no more than hqdefault's 480 px (16:9 inside 480x360): a
        // fifth of hq720's pixels to decode, and only a slight shrink to the drawn width.
        candidates.extend([
            (Poster::Hq, canonical("hqdefault")),
            (Poster::Sd, canonical("sddefault")),
        ]);
        candidates.extend(renderer);
        candidates.push((Poster::Hd, canonical("hq720")));
    } else if max_width == 0 || max_width > 320 {
        candidates.extend(renderer);
        candidates.extend([
            (Poster::Hd, canonical("hq720")),
            (Poster::Sd, canonical("sddefault")),
            (Poster::Hq, canonical("hqdefault")),
        ]);
    }
    candidates.push((Poster::Medium, canonical("mqdefault")));
    Some((id.to_owned(), candidates))
}

const MISSING_VARIANT_TTL: Duration = Duration::from_secs(600);
type MissingPoster = (String, Poster);

fn missing_posters() -> &'static Mutex<VecDeque<(MissingPoster, Instant)>> {
    static MISSING: OnceLock<Mutex<VecDeque<(MissingPoster, Instant)>>> = OnceLock::new();
    MISSING.get_or_init(Default::default)
}

async fn fetch_image(url: &str, max_width: u32) -> Result<RgbaImage> {
    let Some((id, candidates)) = poster_candidates(url, max_width) else {
        let bytes = client().get_bytes(url).await?;
        return decode_worker(bytes, max_width, None).await;
    };
    let mut failure = None;
    for (kind, candidate) in candidates {
        let missing_key = (
            if kind == Poster::Renderer {
                candidate.clone()
            } else {
                id.clone()
            },
            kind,
        );
        {
            let mut missing = missing_posters().lock().unwrap_or_else(|e| e.into_inner());
            while missing
                .front()
                .is_some_and(|(_, at)| at.elapsed() >= MISSING_VARIANT_TTL)
            {
                missing.pop_front();
            }
            if missing.iter().any(|(key, _)| key == &missing_key) {
                continue;
            }
        }
        match client().get_bytes_if_present(&candidate).await {
            Ok(Some(bytes)) => match decode_worker(bytes, max_width, Some(kind)).await {
                Ok(img) => return Ok(img),
                Err(Error::Parse(message)) if message == "Thumbnail variant is a placeholder" => {}
                Err(e) => {
                    failure = Some(e);
                    continue;
                }
            },
            Ok(None) => {}
            Err(e @ Error::Fixture(_)) => {
                failure = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        }
        if kind != Poster::Medium {
            let mut missing = missing_posters().lock().unwrap_or_else(|e| e.into_inner());
            if missing.len() >= 2048 {
                missing.pop_front();
            }
            missing.push_back((missing_key, Instant::now()));
        }
    }
    Err(failure.unwrap_or_else(|| Error::Network("Video thumbnail is unavailable".into())))
}

async fn decode_worker(
    bytes: bytes::Bytes,
    max_width: u32,
    poster: Option<Poster>,
) -> Result<RgbaImage> {
    tokio::task::spawn_blocking(move || {
        let trace = std::env::var_os("NT_IMAGE_TRACE").is_some();
        let started = Instant::now();
        let cpu = trace.then(decode_thread_cpu_ms);
        let decoded = decode_image(&bytes, max_width, poster);
        if let Some(cpu) = cpu {
            eprintln!(
                "image decode: {:?} bytes={} width={max_width} ms={:.2} thread_cpu_ms={:.2}",
                poster,
                bytes.len(),
                started.elapsed().as_secs_f64() * 1000.0,
                decode_thread_cpu_ms() - cpu
            );
        }
        decoded
    })
    .await
    .map_err(|_| Error::Cancelled)?
}

#[cfg(target_os = "linux")]
fn decode_thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: time is writable and the clock belongs to the calling decoder thread.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut time) };
    if status == 0 {
        time.tv_sec as f64 * 1000.0 + time.tv_nsec as f64 / 1e6
    } else {
        f64::NAN
    }
}

#[cfg(not(target_os = "linux"))]
fn decode_thread_cpu_ms() -> f64 {
    f64::NAN
}

/// Decoded bytes currently held by the cache.
pub fn cached_bytes() -> usize {
    cache().lock().map(|c| c.bytes).unwrap_or(0)
}

fn decode_image(bytes: &[u8], max_width: u32, poster: Option<Poster>) -> Result<RgbaImage> {
    let mut img =
        image::load_from_memory(bytes).map_err(|e| Error::Parse(format!("image: {e}")))?;
    if let Some(kind) = poster {
        if kind != Poster::Medium && img.width() < 320 {
            return Err(Error::Parse("Thumbnail variant is a placeholder".into()));
        }
        let crop_h = (u64::from(img.width()) * 9 / 16).max(1) as u32;
        if img.height() > crop_h {
            img = img.crop_imm(0, (img.height() - crop_h) / 2, img.width(), crop_h);
        }
    }
    let img = if max_width > 0 && img.width() > max_width {
        let h =
            (u64::from(img.height()) * u64::from(max_width) / u64::from(img.width())).max(1) as u32;
        let filter = if img.width() >= max_width.saturating_mul(2) {
            image::imageops::FilterType::Lanczos3
        } else {
            image::imageops::FilterType::Triangle
        };
        img.resize_exact(max_width, h, filter)
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
            c.slots.insert(
                k("a"),
                Entry {
                    slot: a.clone(),
                    bytes: None,
                },
            );
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
