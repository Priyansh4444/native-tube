use rune::{App, AppSpec, Cx, El, LiveText, text};

fn view(value: &LiveText, _: &mut Cx<LiveText>) -> El {
    text("A single-line placeholder")
        .w(120.0)
        .wrap()
        .live_text(value)
}

fn main() {
    rune::set_app_id("nttest-rune");
    App::new(AppSpec::new(
        "Wrapped LiveText",
        (640.0, 480.0),
        || LiveText::new("A value that needs more than one line in this box"),
        view,
    ))
    .run();
}
