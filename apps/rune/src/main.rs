//! NativeTube on Rune.

mod app;
mod images;
mod pages;
mod scroller;
mod theme;
mod video;
mod view;

use rune::{AppSpec, FrameOutcome, KeyEvent, Tx};

fn poll(s: &mut app::App, _clock: f64, tx: &mut Tx) -> bool {
    s.poll(tx)
}

fn on_key(s: &mut app::App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.on_key(ev, tx)
}

fn main() {
    nt_core::metrics::init("rune");
    nt_core::autopilot::start();
    rune::set_app_id(Box::leak(std::env::var("NT_APP_ID").unwrap_or_else(|_| "nt-rune".to_string()).into_boxed_str()));
    let spec = AppSpec {
        min_size: Some((640.0, 480.0)),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("NativeTube", (1280.0, 800.0), app::init, view::view)
    };
    let app = rune::App::new(spec);
    app::set_handle(app.handle());
    if let Some(h) = app::handle() {
        let h = h.clone();
        nt_core::autopilot::set_waker(move || h.update_if(|s, tx| s.drain_autopilot(tx)));
    }
    app.on_frame_encoded(|f| {
        // Encoded, submitted and presented: the closest hook to presentation Rune offers.
        if f.outcome() == FrameOutcome::Drawn {
            nt_core::metrics::frame();
            app::flush_marks();
        }
    })
    .run();
}
