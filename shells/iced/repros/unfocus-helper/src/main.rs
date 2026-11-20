//! Demonstrate the public advanced operation for clearing focus.
use iced::widget::{button, column, operation, text, text_input};
use iced::{Element, Size, Task, window};

#[derive(Clone, Debug)]
enum Message {
    Input(String),
    Focus,
    Blur,
    Focused(bool),
}

#[derive(Default)]
struct State {
    input: String,
    focused: Option<bool>,
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    match message {
        Message::Input(s) => state.input = s,
        Message::Focus => {
            return operation::focus("field")
                .chain(operation::is_focused("field").map(Message::Focused));
        }
        Message::Blur => {
            let blur = iced::advanced::widget::operate(
                iced::advanced::widget::operation::focusable::unfocus(),
            );
            return blur.chain(operation::is_focused("field").map(Message::Focused));
        }
        Message::Focused(focused) => {
            state.focused = Some(focused);
            eprintln!("REPRO_FOCUSED {focused}");
        }
    }
    Task::none()
}

fn view(state: &State) -> Element<'_, Message> {
    column![
        text_input("Type here", &state.input)
            .id("field")
            .on_input(Message::Input)
            .on_submit(Message::Blur)
            .width(280),
        button("Focus the field").on_press(Message::Focus),
        text(format!("Last operation: {:?}", state.focused)),
    ]
    .padding(96)
    .spacing(12)
    .into()
}

fn main() -> iced::Result {
    iced::application(State::default, update, view)
        .window(window::Settings {
            size: Size::new(1280.0, 800.0),
            platform_specific: window::settings::PlatformSpecific {
                application_id: std::env::var("NT_APP_ID").unwrap_or_default(),
                ..Default::default()
            },
            ..Default::default()
        })
        .run()
}
