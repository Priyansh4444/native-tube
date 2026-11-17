//! The bare Rune shell: one window, one label.

use rune::{AppSpec, Cx, El, Family, TextStyle, col, text};

const ROBOTO: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Regular.ttf");

fn view(_: &(), _: &mut Cx<()>) -> El {
    col()
        .w_full()
        .h_full()
        .center()
        .fill(rune::color::hex("#0f0f0f").unwrap_or(rune::Color::BLACK))
        .child(
            text("NativeTube")
                .font(TextStyle::new(14.0, 400.0).family(Family::Named("Roboto")))
                .color(rune::color::hex("#f1f1f1").unwrap_or(rune::Color::BLACK)),
        )
}

fn main() {
    rune::set_app_id(Box::leak(
        std::env::var("NT_APP_ID")
            .unwrap_or_else(|_| "nt-shell-rune".to_string())
            .into_boxed_str(),
    ));
    rune::run(AppSpec {
        fonts: &[ROBOTO],
        ..AppSpec::new("NativeTube shell", (1280.0, 800.0), || (), view)
    });
}
