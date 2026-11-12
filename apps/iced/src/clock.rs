//! One monotonic clock, in seconds, for input timestamps and frame times alike.

use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

pub fn init() {
    EPOCH.get_or_init(Instant::now);
}

pub fn secs(t: Instant) -> f64 {
    t.saturating_duration_since(*EPOCH.get_or_init(Instant::now)).as_secs_f64()
}

pub fn now() -> f64 {
    secs(Instant::now())
}
