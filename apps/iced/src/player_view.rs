//! The watch page's player and the blocks under it: the picture with its auto-hiding control
//! bar, scrubber, settings menu, centre bezel and "Up next" card; the title and owner row; and
//! the description with links, hashtags and timestamps.

use crate::app::{App, Link, MenuPage, Message, SPEEDS, SleepTimer, Watch};
use crate::icons::{Icon, icon};
use crate::interaction::{Control, action};
use crate::theme::{self as t};
use crate::video::VideoProgram;
use crate::view::{El, subscribe_button, txt};
use crate::widgets::{Seg, auto_hide, bezel, clamp, rich, seek_bar, seg, spinner, thumb};
use iced::widget::{Space, button, column, container, mouse_area, pin, row, shader, slider, stack};
use iced::{Alignment, Fill, Length, Padding};
use nt_core::{Status, WatchInfo, format, library};
use std::ops::Range;

/// A 40 px control-bar button with a 24 px icon.
fn ctl<'a>(i: Icon, msg: Message) -> El<'a> {
    let (kind, label) = match i {
        Icon::PlayArrow => ("play", "Play"),
        Icon::Pause => ("play", "Pause"),
        Icon::Replay => ("play", "Replay"),
        Icon::SkipNext => ("next", "Next"),
        Icon::ChevronLeft => ("previous", "Previous"),
        Icon::VolumeOff => ("mute", "Unmute"),
        Icon::VolumeDown | Icon::VolumeUp => ("mute", "Mute"),
        Icon::Settings => ("settings", "Settings"),
        Icon::WidthWide => ("theatre", "Theatre"),
        Icon::Fullscreen => ("fullscreen", "Fullscreen"),
        Icon::FullscreenExit => ("fullscreen", "Exit fullscreen"),
        Icon::ClosedCaption => ("captions", "CC"),
        _ => ("button", ""),
    };
    action(
        button(container(icon(i, 24.0, t::TEXT)).center(Fill))
            .width(40)
            .height(40)
            .padding(0)
            .style(t::control_button)
            .on_press(msg.clone()),
        kind,
        label,
        msg,
    )
}

fn speed_label(s: f64) -> String {
    if (s - 1.0).abs() < 0.01 {
        "Normal".into()
    } else {
        format!("{}", (s * 100.0).round() / 100.0)
    }
}

pub fn player<'a>(app: &'a App, w: &'a Watch, pw: f32, ph: f32, radius: f32) -> El<'a> {
    let st = &app.play;
    let mine = app.playing_page == app.top().map(|p| p.id);
    let mut layers: Vec<El<'a>> = vec![
        container(Space::new())
            .width(pw)
            .height(ph)
            .style(t::black(radius))
            .into(),
    ];
    if let (Some(f), true) = (&app.frame, mine) {
        let aspect = st
            .aspect()
            .unwrap_or(f.width as f32 / f.height.max(1) as f32);
        let (fw, fh) = crate::surface::fit(pw, ph, aspect);
        if let Some(p) = crate::ext::existing_player() {
            p.set_target_size((fw * app.scale) as u32, (fh * app.scale) as u32);
        }
        // Round the picture's corners only where it touches the box's.
        let r = if (fw - pw).abs() < 2.0 && (fh - ph).abs() < 2.0 {
            radius
        } else {
            0.0
        };
        let decoded = app.has_decoded_frame();
        layers.push(
            container(
                shader(VideoProgram::<false> {
                    frame: Some(f.clone()),
                    radius: r * app.scale,
                    first_frame: decoded.then_some(&w.first_frame),
                    playback_mark: decoded.then_some(&app.want_playback_mark),
                })
                .width(fw)
                .height(fh),
            )
            .width(pw)
            .height(ph)
            .center(Length::Fixed(pw))
            .into(),
        );
    } else {
        if let Some(p) = crate::ext::existing_player() {
            p.set_target_size((pw * app.scale) as u32, (ph * app.scale) as u32);
        }
        let physical_width = pw * app.scale;
        let handle = app
            .images
            .cached(&w.video.thumbnail, physical_width)
            .or_else(|| app.images.get(&w.video.thumbnail, physical_width));
        if let Some(handle) = handle {
            layers.push(thumb(Some(handle), pw, ph, radius).cover().into());
        }
    }
    let failed = match (&st.status, &app.player_error) {
        (_, Some(e)) => Some(e.clone()),
        (Status::Failed(_), _) => Some("This video can't be played right now.".to_string()),
        _ => None,
    };
    if let Some(e) = failed {
        layers.push(
            container(
                column![
                    icon(Icon::Error, 48.0, t::TEXT),
                    txt("Something went wrong", t::HEADING),
                    txt(e, t::BODY).color(t::TEXT_2).align_x(Alignment::Center),
                    action(
                        button(txt("Retry", t::BODY_M))
                            .padding(Padding {
                                top: 8.0,
                                bottom: 8.0,
                                left: 16.0,
                                right: 16.0
                            })
                            .style(t::pill)
                            .on_press(Message::RetryPlayback),
                        "retry",
                        "Retry",
                        Message::RetryPlayback,
                    )
                ]
                .spacing(12)
                .align_x(Alignment::Center)
                .max_width((pw - 48.0).max(100.0)),
            )
            .width(pw)
            .height(ph)
            .center(Length::Fixed(pw))
            .into(),
        );
    } else if mine
        && (st.status == Status::Loading
            || st.buffering
            || (app.frame.is_none() && st.status != Status::Ready))
    {
        layers.push(
            container(spinner(48.0))
                .width(pw)
                .height(ph)
                .center(Length::Fixed(pw))
                .into(),
        );
    }
    let picture = Control::new(stack(layers).width(pw).height(ph))
        .software_only(&app.surface_visible)
        .element();
    let mut layers = vec![picture];
    layers.push(container(bezel(app.bezel)).width(pw).height(ph).into());
    if let Some(up) = &app.up_next
        && Some(up.page) == app.playing_page
        && let Some(v) = app.next_video()
    {
        layers.push(up_next(app, v, up.remaining, up.counting, pw, ph));
    }
    if app.stats {
        layers.push(pin(stats(app, w, pw)).x(12).y(12).into());
    }
    let base = Control::new(
        mouse_area(stack(layers).width(pw).height(ph))
            .on_press(Message::ClickVideo)
            .on_right_press(Message::PlayerContext)
            .on_double_click(Message::ToggleFullscreen),
    )
    .target("player", w.video.title.as_str())
    .element();

    let paused = st.paused || matches!(st.status, Status::Ended | Status::Idle);
    let controls = controls(app, w, pw);
    let mut overlay_layers: Vec<El<'a>> = vec![
        column![
            Space::new().width(Fill).height(Fill),
            container(controls).width(Fill).style(t::scrim)
        ]
        .width(pw)
        .height(ph)
        .into(),
    ];
    if let Some(page) = app.menu {
        overlay_layers.push(settings_menu(app, page, pw, ph));
    }
    let overlay = stack(overlay_layers).width(pw).height(ph);
    auto_hide(
        base,
        overlay,
        paused || app.menu.is_some() || app.player_context.is_some() || app.test_controls,
    )
    .on_change(Message::ControlsShown)
    .into()
}

fn stats<'a>(app: &'a App, w: &'a Watch, pw: f32) -> El<'a> {
    let state = &app.play;
    let render = crate::ext::existing_player()
        .map(|p| p.render_stats())
        .unwrap_or_default();
    let mean = render.total_render_ms / render.frames.max(1) as f64;
    let resolution = state
        .video_size
        .map_or_else(|| "—".into(), |(w, h)| format!("{w} × {h}"));
    let frame = app
        .frame
        .as_ref()
        .map_or_else(|| "—".into(), |f| format!("{} × {}", f.width, f.height));
    let close = action(
        button(icon(Icon::Close, 18.0, t::TEXT))
            .padding(4)
            .style(t::control_button)
            .on_press(Message::Stats(false)),
        "menu_item",
        "Close stats",
        Message::Stats(false),
    );
    let data = [
        ("Video ID", w.video.id.clone()),
        ("Source / render size", format!("{resolution} / {frame}")),
        (
            "Playback",
            format!(
                "{} / {} · {:.2}×",
                format::clock(state.position),
                format::clock(state.duration),
                state.speed
            ),
        ),
        ("Buffer", format!("{:.1} s", state.buffered_ahead)),
        ("Frames rendered", render.frames.to_string()),
        (
            "Render time",
            format!("{mean:.2} ms avg / {:.2} ms max", render.max_render_ms),
        ),
    ];
    let mut col = column![
        row![
            txt("Stats for nerds", t::BODY_M),
            Space::new().width(Fill),
            close
        ]
        .align_y(Alignment::Center)
    ];
    for (label, value) in data {
        col = col.push(
            row![
                txt(label, t::META_S).width(126).color(t::TEXT_2),
                txt(value, t::META_S)
            ]
            .spacing(8),
        );
    }
    let panel = container(col.spacing(4))
        .padding([8, 12])
        .width((pw - 24.0).min(400.0))
        .style(|_| container::Style {
            background: Some(iced::Color::from_rgba8(0, 0, 0, 0.72).into()),
            border: iced::Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    Control::new(iced::widget::opaque(panel)).cover().element()
}

/// The CC button: YouTube's red underline marks captions as on.
fn captions_button<'a>(on: bool) -> El<'a> {
    let msg = Message::ToggleCaptions;
    let underline = container(Space::new())
        .width(20)
        .height(3)
        .style(move |_: &iced::Theme| container::Style {
            background: on.then_some(t::RED.into()),
            border: iced::Border {
                radius: 1.5.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    action(
        button(
            column![
                Space::new().height(6),
                icon(Icon::ClosedCaption, 24.0, t::TEXT),
                underline
            ]
            .align_x(Alignment::Center)
            .width(Fill),
        )
        .width(40)
        .height(40)
        .padding(0)
        .style(t::control_button)
        .on_press(msg.clone()),
        "captions",
        "CC",
        msg,
    )
}

/// What the captions setting reads: "Off", the track, or "track >> translation".
fn captions_label(st: &nt_core::PlaybackState) -> String {
    match &st.selected_captions {
        None => "Off".into(),
        Some((i, tl)) => {
            let track = st.captions.get(*i).map_or("On", |c| c.label.as_str());
            match tl.as_ref().and_then(|code| {
                st.caption_languages
                    .iter()
                    .find(|l| &l.code == code)
                    .map(|l| l.label.as_str())
            }) {
                Some(lang) => format!("{track} >> {lang}"),
                None => track.to_string(),
            }
        }
    }
}

fn controls<'a>(app: &'a App, w: &'a Watch, pw: f32) -> El<'a> {
    let st = &app.play;
    let played = if st.duration > 0.0 {
        (st.position / st.duration) as f32
    } else {
        0.0
    };
    let buffered = if st.duration > 0.0 {
        ((st.position + st.buffered_ahead) / st.duration) as f32
    } else {
        0.0
    };
    let paused = st.paused || st.status == Status::Ended;
    let play_icon = if st.status == Status::Ended {
        Icon::Replay
    } else if paused {
        Icon::PlayArrow
    } else {
        Icon::Pause
    };
    let play_msg = if st.status == Status::Ended {
        Message::Replay
    } else {
        Message::PlayPause
    };
    let vol_icon = if st.muted || st.volume <= 0.0 {
        Icon::VolumeOff
    } else if st.volume < 50.0 {
        Icon::VolumeDown
    } else {
        Icon::VolumeUp
    };
    let mut volume = row![ctl(vol_icon, Message::ToggleMute)].align_y(Alignment::Center);
    if app.volume_hover {
        volume = volume.push(
            container(
                slider(
                    0.0..=100.0,
                    if st.muted { 0.0 } else { st.volume as f32 },
                    Message::SetVolume,
                )
                .step(1.0_f32)
                .style(t::volume)
                .width(52),
            )
            .padding(Padding {
                left: 4.0,
                right: 8.0,
                ..Padding::ZERO
            }),
        );
    }
    let volume = mouse_area(volume)
        .on_enter(Message::VolumeHover(true))
        .on_exit(Message::VolumeHover(false));
    let time: El<'a> = if st.is_live {
        row![
            container(Space::new())
                .width(8)
                .height(8)
                .style(|_: &iced::Theme| {
                    container::Style {
                        background: Some(t::RED.into()),
                        border: iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            txt("LIVE", t::BODY_M)
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else {
        txt(
            format!(
                "{} / {}",
                format::clock(st.position),
                format::clock(st.duration)
            ),
            t::TIME,
        )
        .into()
    };
    let has_next = app.next_video().is_some();
    let has_previous = w
        .playlist
        .as_ref()
        .is_some_and(|p| p.previous_video().is_some());
    let mut left = row![ctl(play_icon, play_msg)].align_y(Alignment::Center);
    if has_previous && pw >= 420.0 {
        left = left.push(ctl(Icon::ChevronLeft, Message::PreviousVideo));
    }
    if has_next && pw >= 420.0 {
        left = left.push(ctl(Icon::SkipNext, Message::UpNextPlay));
    }
    left = left.push(volume).push(Space::new().width(8)).push(time);
    let chapter = w
        .info
        .as_ref()
        .filter(|_| !st.is_live)
        .and_then(|i| i.chapter_at(st.position));
    if let Some(c) = chapter.filter(|_| pw >= 560.0) {
        let label = row![
            txt("\u{2022}", t::TIME).color(t::TEXT_2),
            container(
                txt(c.title.as_str(), t::TIME)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .color(t::TEXT)
            )
            .max_width((pw * 0.3).max(80.0))
            .clip(true),
            icon(Icon::ChevronRight, 16.0, t::TEXT),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        left = left.push(Space::new().width(8)).push(action(
            button(label)
                .padding([4, 6])
                .style(t::control_button)
                .on_press(Message::ShowChapters),
            "chapter",
            c.title.as_str(),
            Message::ShowChapters,
        ));
    }
    let mut right = row![].align_y(Alignment::Center);
    if !st.captions.is_empty() {
        right = right.push(captions_button(st.selected_captions.is_some()));
    }
    right = right.push(ctl(
        Icon::Settings,
        Message::Menu(match app.menu {
            Some(_) => None,
            None => Some(MenuPage::Main),
        }),
    ));
    if !app.fullscreen {
        right = right.push(ctl(Icon::WidthWide, Message::ToggleTheatre));
    }
    right = right.push(ctl(
        if app.fullscreen {
            Icon::FullscreenExit
        } else {
            Icon::Fullscreen
        },
        Message::ToggleFullscreen,
    ));
    let bar: El<'a> = if st.is_live || st.duration <= 0.0 {
        Space::new().height(16).into()
    } else {
        let (chapters, heat) = w
            .info
            .as_ref()
            .map_or((&[][..], &[][..]), |i| (&i.chapters[..], &i.heatmap[..]));
        let mut bar = seek_bar(played, buffered, st.duration, Message::SeekFrac)
            .chapters(chapters)
            .heatmap(heat);
        if st.storyboard.is_some() {
            bar = bar.storyboard(app.scrub_preview.clone(), Message::ScrubHover);
        }
        Control::new(bar).target("scrubber", "Seek").element()
    };
    column![
        Space::new().height(48),
        container(bar).padding(Padding {
            left: 12.0,
            right: 12.0,
            ..Padding::ZERO
        }),
        row![left, Space::new().width(Fill), right]
            .height(48)
            .align_y(Alignment::Center)
            .padding(Padding {
                left: 8.0,
                right: 8.0,
                ..Padding::ZERO
            })
    ]
    .width(Fill)
    .into()
}

fn settings_menu<'a>(app: &'a App, page: MenuPage, pw: f32, ph: f32) -> El<'a> {
    let st = &app.play;
    let row_height = if page == MenuPage::Sleep { 36.0 } else { 40.0 };
    let quality = crate::ext::existing_player().and_then(|p| p.quality());
    let item = |label: String,
                left: El<'a>,
                value: Option<String>,
                chevron: bool,
                msg: Message|
     -> El<'a> {
        let mut r = row![left, Space::new().width(Fill)]
            .spacing(12)
            .align_y(Alignment::Center);
        if let Some(v) = value {
            r = r.push(txt(v, t::BODY));
        }
        if chevron {
            r = r.push(icon(Icon::ChevronRight, 20.0, t::TEXT));
        }
        let control = button(r)
            .width(Fill)
            .height(row_height)
            .padding(Padding {
                top: 10.0,
                bottom: 10.0,
                left: 16.0,
                right: 12.0,
            })
            .style(t::menu_row(false))
            .on_press(msg.clone());
        action(control, "menu_item", label, msg)
    };
    let labelled = |i: Icon, label: &'static str| -> El<'a> {
        row![icon(i, 24.0, t::TEXT), txt(label, t::BODY)]
            .spacing(16)
            .align_y(Alignment::Center)
            .into()
    };
    let check_row = |label: String, on: bool, msg: Message| -> El<'a> {
        let mark: El<'a> = if on {
            icon(Icon::Check, 20.0, t::TEXT)
        } else {
            Space::new().width(20).into()
        };
        item(
            label.clone(),
            row![mark, txt(label, t::BODY)]
                .spacing(16)
                .align_y(Alignment::Center)
                .into(),
            None,
            false,
            msg,
        )
    };
    let header = |label: &'static str, back: MenuPage| -> El<'a> {
        item(
            label.into(),
            row![
                icon(Icon::ChevronLeft, 20.0, t::TEXT),
                txt(label, t::BODY_M)
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
            None,
            false,
            Message::Menu(Some(back)),
        )
    };
    let rows: Vec<El<'a>> =
        match page {
            MenuPage::Main => {
                let mut v = vec![item(
                    "Autoplay".into(),
                    labelled(Icon::SkipNext, "Autoplay"),
                    Some(if app.prefs.autoplay { "On" } else { "Off" }.into()),
                    false,
                    Message::ToggleAutoplay,
                )];
                if !st.captions.is_empty() {
                    v.push(item(
                        "Subtitles/CC".into(),
                        labelled(Icon::ClosedCaption, "Subtitles/CC"),
                        Some(captions_label(st)),
                        true,
                        Message::Menu(Some(MenuPage::Captions)),
                    ));
                }
                if !st.is_live {
                    v.push(item(
                        "Playback speed".into(),
                        labelled(Icon::Speed, "Playback speed"),
                        Some(speed_label(st.speed)),
                        true,
                        Message::Menu(Some(MenuPage::Speed)),
                    ));
                }
                if !st.audio_tracks.is_empty() {
                    let label = st
                        .audio_tracks
                        .iter()
                        .find(|a| Some(&a.id) == st.audio_track.as_ref())
                        .map(|a| a.label.clone());
                    v.push(item(
                        "Audio track".into(),
                        labelled(Icon::VolumeUp, "Audio track"),
                        label,
                        true,
                        Message::Menu(Some(MenuPage::Audio)),
                    ));
                }
                if st.normalization_supported {
                    v.push(item(
                        "Stable volume".into(),
                        labelled(Icon::VolumeUp, "Stable volume"),
                        Some(if st.normalize { "On" } else { "Off" }.into()),
                        false,
                        Message::Normalize,
                    ));
                }
                v.push(item(
                    "Loop".into(),
                    labelled(Icon::Replay, "Loop"),
                    Some(if st.looping { "On" } else { "Off" }.into()),
                    false,
                    Message::Loop,
                ));
                let q = match quality {
                    None if st.height > 0 => format!("Auto ({}p)", st.height),
                    None => "Auto".into(),
                    Some(h) => format!("{h}p"),
                };
                v.push(item(
                    "Quality".into(),
                    labelled(Icon::Tune, "Quality"),
                    Some(q),
                    true,
                    Message::Menu(Some(MenuPage::Quality)),
                ));
                v.push(item(
                    "Sleep timer".into(),
                    labelled(Icon::Schedule, "Sleep timer"),
                    Some(app.sleep.label()),
                    true,
                    Message::Menu(Some(MenuPage::Sleep)),
                ));
                v
            }
            MenuPage::Speed => {
                let mut v = vec![header("Playback speed", MenuPage::Main)];
                v.extend(SPEEDS.iter().map(|s| {
                    check_row(
                        speed_label(*s),
                        (s - st.speed).abs() < 0.01,
                        Message::SetSpeed(*s),
                    )
                }));
                v
            }
            MenuPage::Captions => {
                let mut v = vec![
                    header("Subtitles/CC", MenuPage::Main),
                    check_row(
                        "Off".into(),
                        st.selected_captions.is_none(),
                        Message::SetCaptions(None),
                    ),
                ];
                let selected = st.selected_captions.as_ref().map(|(i, _)| *i);
                v.extend(st.captions.iter().enumerate().map(|(i, c)| {
                    check_row(
                        c.label.clone(),
                        selected == Some(i),
                        Message::SetCaptions(Some((i, None))),
                    )
                }));
                if let Some(i) = selected
                    && st.captions.get(i).is_some_and(|c| c.translatable)
                    && !st.caption_languages.is_empty()
                {
                    let current = st
                        .selected_captions
                        .as_ref()
                        .and_then(|(_, tl)| tl.as_ref())
                        .and_then(|code| st.caption_languages.iter().find(|l| &l.code == code))
                        .map_or("Off".to_string(), |l| l.label.clone());
                    v.push(item(
                        "Auto-translate".into(),
                        row![Space::new().width(20), txt("Auto-translate", t::BODY)]
                            .spacing(16)
                            .align_y(Alignment::Center)
                            .into(),
                        Some(current),
                        true,
                        Message::Menu(Some(MenuPage::Translate)),
                    ));
                }
                v
            }
            MenuPage::Translate => {
                let mut v = vec![header("Auto-translate", MenuPage::Captions)];
                if let Some((i, tl)) = &st.selected_captions {
                    let i = *i;
                    v.extend(st.caption_languages.iter().map(|l| {
                        check_row(
                            l.label.clone(),
                            tl.as_deref() == Some(l.code.as_str()),
                            Message::SetCaptions(Some((i, Some(l.code.clone())))),
                        )
                    }));
                }
                v
            }
            MenuPage::Audio => {
                let mut v = vec![header("Audio track", MenuPage::Main)];
                v.extend(st.audio_tracks.iter().map(|audio| {
                    check_row(
                        if audio.auto_dubbed {
                            format!("{} (auto-dubbed)", audio.label)
                        } else {
                            audio.label.clone()
                        },
                        Some(&audio.id) == st.audio_track.as_ref(),
                        Message::SetAudio(audio.id.clone()),
                    )
                }));
                v
            }
            MenuPage::Sleep => {
                let mut v = vec![header("Sleep timer", MenuPage::Main)];
                v.extend(SleepTimer::OPTIONS.into_iter().map(|timer| {
                    check_row(timer.label(), app.sleep == timer, Message::Sleep(timer))
                }));
                v
            }
            MenuPage::Quality => {
                let mut v = vec![header("Quality", MenuPage::Main)];
                for h in st.heights.iter().rev() {
                    v.push(check_row(
                        format!("{h}p"),
                        quality == Some(*h),
                        Message::SetQuality(Some(*h)),
                    ));
                }
                let auto = if st.height > 0 && quality.is_none() {
                    format!("Auto ({}p)", st.height)
                } else {
                    "Auto".into()
                };
                v.push(check_row(
                    auto,
                    quality.is_none(),
                    Message::SetQuality(None),
                ));
                v
            }
        };
    let n = rows.len() as f32;
    let mh = (n * row_height + 16.0).min((ph - 72.0).max(80.0));
    let wide = matches!(
        page,
        MenuPage::Captions | MenuPage::Translate | MenuPage::Audio
    ) || st.selected_captions.is_some();
    let mw = (if wide { 300.0_f32 } else { 260.0 }).min(pw - 24.0);
    let menu = container(
        Control::new(
            iced::widget::scrollable(column(rows))
                .height(Length::Fixed(mh - 16.0))
                .width(Fill),
        )
        .scroll_scope()
        .element(),
    )
    .style(t::menu)
    .padding(Padding {
        top: 8.0,
        bottom: 8.0,
        ..Padding::ZERO
    })
    .width(mw)
    .height(mh);
    pin(Control::new(iced::widget::opaque(menu)).cover().element())
        .x((pw - mw - 12.0).max(0.0))
        .y((ph - mh - 64.0).max(0.0))
        .width(pw)
        .height(ph)
        .into()
}

fn up_next<'a>(
    app: &'a App,
    v: &'a nt_core::Video,
    remaining: u32,
    counting: bool,
    pw: f32,
    ph: f32,
) -> El<'a> {
    let cw = 420.0_f32.min(pw - 32.0).max(200.0);
    let tw = (cw * 0.38).min(160.0);
    let th = (tw * 9.0 / 16.0).round();
    let head = if counting {
        format!("Up next in {remaining}")
    } else {
        "Up next".to_string()
    };
    let card = container(
        column![
            txt(head, t::BODY).color(t::TEXT_2),
            row![
                thumb(
                    app.images.get(&v.thumbnail, tw * app.scale),
                    tw,
                    th,
                    t::R_COMPACT
                )
                .cover(),
                column![
                    clamp(v.title.as_str(), t::BODY_M, 2).color(t::TEXT),
                    clamp(v.channel.as_str(), t::META_S, 1).color(t::TEXT_2),
                ]
                .spacing(4)
                .width(Fill)
            ]
            .spacing(12),
            row![
                action(
                    button(txt("Cancel", t::BODY_M))
                        .padding([8, 16])
                        .style(t::pill)
                        .on_press(Message::UpNextCancel),
                    "menu_item",
                    "Cancel",
                    Message::UpNextCancel,
                ),
                action(
                    button(txt("Play now", t::BODY_M))
                        .padding([8, 16])
                        .style(t::primary)
                        .on_press(Message::UpNextPlay),
                    "menu_item",
                    "Play now",
                    Message::UpNextPlay,
                ),
                Space::new().width(Fill),
                action(
                    button(container(icon(Icon::Replay, 24.0, t::TEXT)).center(Fill))
                        .width(36)
                        .height(36)
                        .padding(0)
                        .style(t::pill)
                        .on_press(Message::Replay),
                    "play",
                    "Replay",
                    Message::Replay,
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12),
    )
    .style(t::overlay_card)
    .padding(16)
    .width(cw);
    container(Control::new(iced::widget::opaque(card)).cover().element())
        .width(pw)
        .height(ph)
        .center(Length::Fixed(pw))
        .into()
}

pub fn watch_info<'a>(app: &'a App, w: &'a Watch, width: f32) -> El<'a> {
    let title = w
        .info
        .as_ref()
        .map(|i| i.title.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(w.video.title.as_str());
    let (name, avatar, subs, ch_id) = match &w.info {
        Some(i) => (
            i.channel.name.as_str(),
            i.channel.avatar.as_ref(),
            i.channel.subscribers.as_deref(),
            i.channel.id.as_str(),
        ),
        None => (
            w.video.channel.as_str(),
            w.video.channel_avatar.as_ref(),
            None,
            w.video.channel_id.as_deref().unwrap_or(""),
        ),
    };
    let av = button(thumb(
        avatar.and_then(|u| app.images.get(u, 40.0 * app.scale)),
        40.0,
        40.0,
        20.0,
    ))
    .padding(0)
    .style(t::bare)
    .on_press(Message::OpenWatchChannel);
    let mut title_row = row![
        container(clamp(name, t::CARD_TITLE, 1).color(t::TEXT).shrink())
            .max_width((width * 0.35).clamp(120.0, 260.0))
    ]
    .spacing(5)
    .align_y(Alignment::Center);
    if w.info
        .as_ref()
        .map_or(w.video.channel_verified, |i| i.channel_verified)
    {
        title_row = title_row.push(icon(Icon::Check, 15.0, t::TEXT_2));
    }
    let mut names = column![action(
        button(title_row)
            .padding(0)
            .style(t::bare)
            .on_press(Message::OpenWatchChannel),
        "channel",
        name,
        Message::OpenWatchChannel,
    )];
    if let Some(s) = subs {
        names = names.push(
            txt(s, t::META_S)
                .wrapping(iced::widget::text::Wrapping::None)
                .color(t::TEXT_2),
        );
    }
    let mut owner = row![av, names].spacing(12).align_y(Alignment::Center);
    if w.info.is_some() && !ch_id.is_empty() {
        owner = owner
            .push(Space::new().width(12))
            .push(subscribe_button(app.is_subscribed(ch_id)));
    }
    let likes = w.info.as_ref().and_then(|i| i.likes.as_deref());
    let like = button(
        row![icon(Icon::ThumbUp, 24.0, t::TEXT)]
            .push(
                likes
                    .map(|l| -> El<'a> { txt(l, t::BODY_M).into() })
                    .unwrap_or_else(|| Space::new().into()),
            )
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .height(36)
    .padding(Padding {
        top: 6.0,
        bottom: 6.0,
        left: 12.0,
        right: 12.0,
    })
    .style(t::pill_half(true))
    .on_press(Message::Toast(
        "Liking needs a YouTube account; NativeTube stays signed out.".into(),
    ));
    let dislike = button(container(icon(Icon::ThumbDown, 24.0, t::TEXT)).center(Fill))
        .width(48)
        .height(36)
        .padding(0)
        .style(t::pill_half(false))
        .on_press(Message::Toast(
            "Disliking needs a YouTube account; NativeTube stays signed out.".into(),
        ));
    let divider = container(Space::new())
        .width(1)
        .height(24)
        .style(t::divider);
    let pill_btn = |i: Icon, label: &'static str, msg: Message| -> El<'a> {
        let control = button(
            row![icon(i, 24.0, t::TEXT), txt(label, t::BODY_M)]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .height(36)
        .padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 12.0,
            right: 16.0,
        })
        .style(t::pill)
        .on_press(msg.clone());
        let kind = if i == Icon::Share {
            "share"
        } else {
            "watch_later"
        };
        action(control, kind, label, msg)
    };
    let saved = library().is_saved(&w.video.id);
    let actions = row![
        row![
            like,
            container(divider)
                .height(36)
                .align_y(Alignment::Center)
                .style(|_: &iced::Theme| container::Style {
                    background: Some(t::FILL.into()),
                    ..Default::default()
                }),
            dislike
        ],
        pill_btn(Icon::Share, "Share", Message::Share),
        pill_btn(
            if saved { Icon::Check } else { Icon::Schedule },
            if saved { "Saved" } else { "Watch later" },
            Message::ToggleLater
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let owner_row: El<'a> = if width >= 760.0 {
        row![owner, Space::new().width(Fill), actions]
            .align_y(Alignment::Center)
            .into()
    } else {
        column![owner, actions].spacing(12).into()
    };
    container(
        column![
            rich(vec![seg(title)], t::HEADING, 0).color(t::TEXT),
            owner_row
        ]
        .spacing(12),
    )
    .padding(Padding {
        top: 12.0,
        bottom: 12.0,
        ..Padding::ZERO
    })
    .into()
}

/// Split a description into plain runs and links (URLs, #hashtags, timestamps). Done once
/// per page load; the view borrows the ranges.
pub fn linkify(text: &str) -> Vec<(Range<usize>, Option<Link>)> {
    let mut out: Vec<(Range<usize>, Option<Link>)> = Vec::new();
    let bytes = text.as_bytes();
    let mut plain_start = 0;
    let mut i = 0;
    let push_link =
        |out: &mut Vec<(Range<usize>, Option<Link>)>, ps: usize, s: usize, e: usize, l: Link| {
            if ps < s {
                out.push((ps..s, None));
            }
            out.push((s..e, Some(l)));
        };
    while i < bytes.len() {
        let at_word_start = i == 0 || !bytes.get(i - 1).is_some_and(|b| b.is_ascii_alphanumeric());
        let rest = text.get(i..).unwrap_or("");
        if at_word_start && (rest.starts_with("http://") || rest.starts_with("https://")) {
            let end = i + rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
            let url = text
                .get(i..end)
                .unwrap_or("")
                .trim_end_matches(['.', ',', ')', '!', '?']);
            let end = i + url.len();
            push_link(&mut out, plain_start, i, end, Link::Url(url.to_string()));
            plain_start = end;
            i = end;
            continue;
        }
        if at_word_start && bytes.get(i) == Some(&b'#') {
            let tag_len = rest
                .get(1..)
                .unwrap_or("")
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len().saturating_sub(1));
            if tag_len > 0 {
                let end = i + 1 + tag_len;
                let tag = text.get(i..end).unwrap_or("").to_string();
                push_link(&mut out, plain_start, i, end, Link::Search(tag));
                plain_start = end;
                i = end;
                continue;
            }
        }
        if at_word_start && bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            // h:mm:ss or m:ss
            let len = rest
                .find(|c: char| !(c.is_ascii_digit() || c == ':'))
                .unwrap_or(rest.len());
            let cand = rest.get(..len).unwrap_or("");
            let parts: Vec<&str> = cand.split(':').collect();
            let ok = (parts.len() == 2 || parts.len() == 3)
                && parts.iter().skip(1).all(|p| p.len() == 2)
                && parts.iter().all(|p| !p.is_empty() && p.len() <= 2);
            if ok {
                let secs = parts
                    .iter()
                    .fold(0.0, |acc, p| acc * 60.0 + p.parse::<f64>().unwrap_or(0.0));
                let end = i + len;
                push_link(&mut out, plain_start, i, end, Link::Seek(secs));
                plain_start = end;
                i = end;
                continue;
            }
        }
        i += text
            .get(i..)
            .and_then(|s| s.chars().next())
            .map_or(1, char::len_utf8);
    }
    if plain_start < text.len() {
        out.push((plain_start..text.len(), None));
    }
    out
}

/// The expanded description's chapter list: thumbnail, title and start time; a click seeks.
fn chapter_list<'a>(app: &'a App, info: &'a WatchInfo) -> El<'a> {
    let rows = info.chapters.iter().map(|c| {
        let img = thumb(
            c.thumbnail
                .as_ref()
                .and_then(|u| app.images.get(u, 88.0 * app.scale)),
            88.0,
            50.0,
            4.0,
        )
        .cover();
        let msg = Message::DescLink(Link::Seek(c.start));
        action(
            button(
                row![
                    img,
                    column![
                        clamp(c.title.as_str(), t::BODY_M, 2).color(t::TEXT),
                        txt(format::clock(c.start), t::META_S).color(t::LINK),
                    ]
                    .spacing(2)
                    .width(Fill)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding(4)
            .style(t::menu_row(false))
            .on_press(msg.clone()),
            "chapter",
            c.title.as_str(),
            msg,
        )
    });
    column![txt("Chapters", t::CARD_TITLE), column(rows).spacing(4)]
        .spacing(8)
        .into()
}

pub fn description<'a>(app: &'a App, info: &'a WatchInfo, w: &'a Watch) -> El<'a> {
    let head: Vec<&str> = [info.views.as_deref(), info.date.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let mut segs: Vec<Seg<'a>> = Vec::with_capacity(w.desc_spans.len());
    let mut links: Vec<Link> = Vec::new();
    for (range, link) in &w.desc_spans {
        let piece = info.description.get(range.clone()).unwrap_or("");
        match link {
            Some(l) => {
                segs.push(seg(piece).link(links.len(), t::LINK));
                links.push(l.clone());
            }
            None => segs.push(seg(piece)),
        }
    }
    let on_link = move |i: usize| match links.get(i) {
        Some(l) => Message::DescLink(l.clone()),
        None => Message::ToggleDesc,
    };
    let mut col = column![txt(head.join("  "), t::BODY_M)]
        .spacing(4)
        .width(Fill);
    if info.description.trim().is_empty() {
        col =
            col.push(txt("No description has been added to this video.", t::BODY).color(t::TEXT_2));
    } else if w.desc_open {
        col = col.push(rich(segs, t::BODY, 0).color(t::TEXT).on_link(on_link));
        if !info.chapters.is_empty() {
            col = col
                .push(Space::new().height(16))
                .push(chapter_list(app, info));
        }
        col = col.push(Space::new().height(8)).push(
            button(txt("Show less", t::BODY_M))
                .padding(0)
                .style(t::bare)
                .on_press(Message::ToggleDesc),
        );
    } else {
        col = col.push(
            rich(segs, t::BODY, 3)
                .color(t::TEXT)
                .suffix("…more", t::MEDIUM, t::TEXT)
                .on_link(on_link)
                .on_press(Message::ToggleDesc),
        );
    }
    let boxed = container(col)
        .width(Fill)
        .padding(12)
        .style(t::fill_box(t::R_THUMB));
    let boxed: El<'a> = if w.desc_open {
        boxed.into()
    } else {
        mouse_area(boxed).on_press(Message::ToggleDesc).into()
    };
    container(boxed)
        .padding(Padding {
            top: 4.0,
            bottom: 4.0,
            ..Padding::ZERO
        })
        .into()
}
