//! Measure the numeric setter against the release library built by the Rune app.
//! Compile with `rustc --edition=2024 -O -L dependency=<deps> --extern rune=<rlib>`
//! using the matching Rune `.rlib` from `apps/rune/target/release/deps`.
//! Each output row reports nanoseconds per call in the order recorded in the receipt.
use rune::{Cx, El, Env, Live, Runtime, el};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

const ITERATIONS: u32 = 4_000_000;

fn view(offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
    el().size(20.0, 20.0).live_offset(offset)
}

fn run(offset: &Live<[f32; 2]>, changing: bool) -> f64 {
    let start = Instant::now();
    for index in 1..=ITERATIONS {
        let y = if changing { index as f32 } else { 0.0 };
        black_box(offset.set(black_box([0.0, y])));
    }
    start.elapsed().as_secs_f64() * 1e9 / f64::from(ITERATIONS)
}

fn main() {
    for _ in 0..9 {
        let empty = Live::new([0.0, 0.0]);
        let attached = Live::new([0.0, 0.0]);
        let mut rt = Runtime::new(attached.clone(), view, (100., 100.), Env::default());
        rt.set_live_waker(Arc::new(|| {}));
        rt.frame(0.);
        // Keep the wake pending throughout the timed burst to isolate coalesced publication.
        assert!(attached.set([0.0, -1.0]));
        assert!(rt.live_pending());
        eprintln!(
            "{:.3} {:.3} {:.3}",
            run(&empty, false),
            run(&empty, true),
            run(&attached, true)
        );
        black_box(rt);
    }
}
