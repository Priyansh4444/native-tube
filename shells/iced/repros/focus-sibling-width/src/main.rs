//! Does a text_input keep focus when a sibling before it changes on focus?
//! Click the field, type: every keystroke that reaches the input prints REPRO_INPUT.
//! MODE=width: the sibling is always a Space whose width goes 0 -> 40 px.
//! MODE=swap:  the sibling is a Space (unfocused) or a container (focused): another widget type.
//! MODE=insert: the sibling is only pushed while focused, so the input's index moves 0 -> 1.
//! MODE=stable: preserve the zero-width child with Row::from_vec.
//! MODE=static: control; the sibling never changes.
use iced::widget::{Row, Space, container, row, text, text_input};
use iced::{Element, Event, Size, Subscription, Task, event, mouse, window};

#[derive(Clone, Debug)]
enum Message {
    Input(String),
    Pressed,
}

#[derive(Default)]
struct State {
    value: String,
    focused: bool,
}

fn mode() -> String {
    std::env::var("MODE").unwrap_or_else(|_| "width".into())
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    match message {
        Message::Input(v) => {
            eprintln!("REPRO_INPUT {v:?}");
            state.value = v;
        }
        // Any press counts as "the field gained focus" (it is the only widget to click).
        Message::Pressed => state.focused = true,
    }
    Task::none()
}

fn view(state: &State) -> Element<'_, Message> {
    let input = text_input("Click and type", &state.value)
        .on_input(Message::Input)
        .width(400);
    let content: Element<'_, Message> = match mode().as_str() {
        "swap" if state.focused => row![container(text(">")).width(40), input].into(),
        "swap" => row![Space::new().width(0), input].into(),
        "insert" if state.focused => row![container(text(">")).width(40), input].into(),
        "insert" => row![input].into(),
        "stable" => Row::from_vec(vec![
            Space::new()
                .width(if state.focused { 40 } else { 0 })
                .into(),
            input.into(),
        ])
        .into(),
        "static" => row![Space::new().width(0), input].into(),
        _ => row![
            Space::new().width(if state.focused { 40 } else { 0 }),
            input
        ]
        .into(),
    };
    container(content).padding(200).into()
}

fn subscription(_: &State) -> Subscription<Message> {
    event::listen_with(|e, _, _| match e {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => Some(Message::Pressed),
        _ => None,
    })
}

fn main() -> iced::Result {
    eprintln!("REPRO_MODE {}", mode());
    iced::application(State::default, update, view)
        .subscription(subscription)
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
