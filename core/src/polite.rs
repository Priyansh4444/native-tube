//! Keeping NativeTube's traffic to YouTube at a human scale.
//!
//! Every request to YouTube's own hosts (Innertube, suggestions, feeds, the
//! player endpoint) passes through one gate that:
//! - allows at most [`MAX_IN_FLIGHT`] requests at once and starts them at least
//!   [`MIN_GAP`] apart (a person clicking around never needs more);
//! - stops *all* YouTube requests for a cooldown when YouTube signals that it
//!   wants less traffic (HTTP 429, a 403, or a "confirm you're not a bot" check),
//!   rather than retrying into it. Requests during a cooldown fail at once with
//!   a message saying when it ends.
//!
//! Thumbnails come from the i.ytimg.com image CDN and only pass the in-flight cap
//! in `images.rs`.

use crate::error::{Error, Result};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, SemaphorePermit};

/// Requests to YouTube in flight at once.
pub const MAX_IN_FLIGHT: usize = 3;
/// Least time between the starts of two requests (so at most ~6 a second, and far fewer in practice).
pub const MIN_GAP: Duration = Duration::from_millis(150);
/// Pause after a 429 without Retry-After, or after a 403.
const COOLDOWN_RATE_LIMITED: Duration = Duration::from_secs(5 * 60);
/// Pause after a bot check: YouTube is suspicious of this IP; back off properly.
const COOLDOWN_BOT_CHECK: Duration = Duration::from_secs(30 * 60);

struct Gate {
    slots: Semaphore,
    next_start: tokio::sync::Mutex<Instant>,
    paused_until: Mutex<Option<(Instant, &'static str)>>,
}

fn gate() -> &'static Gate {
    static G: OnceLock<Gate> = OnceLock::new();
    G.get_or_init(|| Gate {
        slots: Semaphore::new(MAX_IN_FLIGHT),
        next_start: tokio::sync::Mutex::new(Instant::now()),
        paused_until: Mutex::new(None),
    })
}

fn paused() -> Option<Error> {
    let g = gate();
    let mut p = g.paused_until.lock().unwrap_or_else(|e| e.into_inner());
    match *p {
        Some((until, why)) if Instant::now() < until => {
            let left = until - Instant::now();
            Some(Error::Network(format!(
                "YouTube asked for less traffic ({why}); NativeTube is pausing its requests for {} more min",
                left.as_secs().div_ceil(60)
            )))
        }
        Some(_) => {
            *p = None;
            None
        }
        None => None,
    }
}

/// Wait for a turn to talk to YouTube. Hold the permit until the response has been read.
pub async fn turn() -> Result<SemaphorePermit<'static>> {
    if let Some(e) = paused() {
        return Err(e);
    }
    let g = gate();
    let permit = g.slots.acquire().await.map_err(|_| Error::Cancelled)?;
    {
        let mut next = g.next_start.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep(*next - now).await;
        }
        *next = Instant::now() + MIN_GAP;
    }
    // A cooldown may have started while we waited.
    if let Some(e) = paused() {
        return Err(e);
    }
    Ok(permit)
}

fn pause(for_: Duration, why: &'static str) {
    let until = Instant::now() + for_;
    let mut p = gate().paused_until.lock().unwrap_or_else(|e| e.into_inner());
    let later = p.is_none_or(|(cur, _)| until > cur);
    if later {
        *p = Some((until, why));
        eprintln!("nt-core: YouTube signalled {why}; pausing all requests to it for {} min", for_.as_secs() / 60);
    }
}

/// Look at a response status before using it. Turns throttling signals into a cooldown.
pub fn check_status(status: reqwest::StatusCode, retry_after: Option<&reqwest::header::HeaderValue>) -> Result<()> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let wait = retry_after
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(COOLDOWN_RATE_LIMITED)
            .max(Duration::from_secs(60));
        pause(wait, "rate limiting (HTTP 429)");
        return Err(paused().unwrap_or(Error::Network("rate limited".into())));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        pause(COOLDOWN_RATE_LIMITED, "a refusal (HTTP 403)");
        return Err(paused().unwrap_or(Error::Network("refused".into())));
    }
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {status}")));
    }
    Ok(())
}

/// Call with any human-readable refusal YouTube sends inside a successful response.
/// Returns true (and starts a long cooldown) if it is a bot check.
pub fn check_refusal(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    let bot = r.contains("not a bot") || r.contains("unusual traffic") || r.contains("automated");
    if bot {
        pause(COOLDOWN_BOT_CHECK, "a bot check");
    }
    bot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_check_detection() {
        assert!(!check_refusal("Sign in to confirm your age"));
        assert!(!check_refusal("Video unavailable"));
    }
}
