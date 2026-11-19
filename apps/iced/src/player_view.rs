//! The watch page's player and the blocks under it: the picture with its auto-hiding control
//! bar, scrubber, settings menu, centre bezel and "Up next" card; the title and owner row; and
//! the description with links, hashtags and timestamps.

use crate::app::{App, Link, MenuPage, Message, SPEEDS, Watch};
use crate::icons::{Icon, icon};
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
    button(container(icon(i, 24.0, t::TEXT)).center(Fill))
        .width(40)
        .height(40)
        .padding(0)
        .style(t::control_button)
        .on_press(msg)
        .into()
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
            .unwrap_or(f.width as f32 / f.height.max(1) as f32)
            .clamp(0.2, 5.0);
        let (mut fw, mut fh) = (pw, pw / aspect);
        if fh > ph {
            fh = ph;
            fw = ph * aspect;
        }
        let (fw, fh) = (fw.floor().max(1.0), fh.floor().max(1.0));
        if let Some(p) = crate::ext::existing_player() {
            p.set_target_size((fw * app.scale) as u32, (fh * app.scale) as u32);
        }
        // Round the picture's corners only where it touches the box's.
        let r = if (fw - pw).abs() < 2.0 && (fh - ph).abs() < 2.0 {
            radius
        } else {
            0.0
        };
        layers.push(
            container(
                shader(VideoProgram {
                    frame: Some(f.clone()),
                    radius: r * app.scale,
                })
                .width(fw)
                .height(fh),
            )
            .width(pw)
            .height(ph)
            .center(Length::Fixed(pw))
            .into(),
        );
    } else if let Some(p) = crate::ext::existing_player() {
        p.set_target_size((pw * app.scale) as u32, (ph * app.scale) as u32);
    }
    let failed = match (&st.status, &app.player_error) {
        (_, Some(e)) => Some(e.clone()),
        (Status::Failed(e), _) => {
            eprintln!("nt-iced: playback: {e}");
            Some("This video can't be played right now.".to_string())
        }
        _ => None,
    };
    if let Some(e) = failed {
        layers.push(
            container(
                column![
                    icon(Icon::Error, 48.0, t::TEXT),
                    txt("Something went wrong", t::HEADING),
                    txt(e, t::BODY).color(t::TEXT_2).align_x(Alignment::Center),
                    button(txt("Retry", t::BODY_M))
                        .padding(Padding {
                            top: 8.0,
                            bottom: 8.0,
                            left: 16.0,
                            right: 16.0
                        })
                        .style(t::pill)
                        .on_press(Message::RetryPlayback)
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
    } else if mine && (st.status == Status::Loading || st.buffering || app.frame.is_none()) {
        layers.push(
            container(spinner(48.0))
                .width(pw)
                .height(ph)
                .center(Length::Fixed(pw))
                .into(),
        );
    }
    layers.push(container(bezel(app.bezel)).width(pw).height(ph).into());
    if let Some(up) = &app.up_next
        && Some(up.page) == app.playing_page
        && let Some(v) = w.related.first()
    {
        layers.push(up_next(app, v, up.remaining, up.counting, pw, ph));
    }
    let base = mouse_area(stack(layers).width(pw).height(ph))
        .on_press(Message::ClickVideo)
        .on_double_click(Message::ToggleFullscreen);

    let paused = st.paused || matches!(st.status, Status::Ended | Status::Idle);
    let controls = controls(app, pw);
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
    auto_hide(base, overlay, paused || app.menu.is_some()).into()
}

fn controls<'a>(app: &'a App, pw: f32) -> El<'a> {
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
    let has_next = app
        .top()
        .and_then(|p| match &p.body {
            crate::app::Body::Watch(w) => w.related.first(),
            _ => None,
        })
        .is_some();
    let mut left = row![ctl(play_icon, play_msg)].align_y(Alignment::Center);
    if has_next && pw >= 420.0 {
        left = left.push(ctl(Icon::SkipNext, Message::UpNextPlay));
    }
    left = left.push(volume).push(Space::new().width(8)).push(time);
    let mut right = row![ctl(
        Icon::Settings,
        Message::Menu(match app.menu {
            Some(_) => None,
            None => Some(MenuPage::Main),
        })
    )]
    .align_y(Alignment::Center);
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
        seek_bar(played, buffered, st.duration, Message::SeekFrac).into()
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
    let quality = crate::ext::existing_player().and_then(|p| p.quality());
    let item = |left: El<'a>, value: Option<String>, chevron: bool, msg: Message| -> El<'a> {
        let mut r = row![left, Space::new().width(Fill)]
            .spacing(12)
            .align_y(Alignment::Center);
        if let Some(v) = value {
            r = r.push(txt(v, t::BODY));
        }
        if chevron {
            r = r.push(icon(Icon::ChevronRight, 20.0, t::TEXT));
        }
        button(r)
            .width(Fill)
            .height(40)
            .padding(Padding {
                top: 10.0,
                bottom: 10.0,
                left: 16.0,
                right: 12.0,
            })
            .style(t::menu_row(false))
            .on_press(msg)
            .into()
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
            row![mark, txt(label, t::BODY)]
                .spacing(16)
                .align_y(Alignment::Center)
                .into(),
            None,
            false,
            msg,
        )
    };
    let header = |label: &'static str| -> El<'a> {
        item(
            row![
                icon(Icon::ChevronLeft, 20.0, t::TEXT),
                txt(label, t::BODY_M)
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
            None,
            false,
            Message::Menu(Some(MenuPage::Main)),
        )
    };
    let rows: Vec<El<'a>> = match page {
        MenuPage::Main => {
            let mut v = vec![item(
                labelled(Icon::SkipNext, "Autoplay"),
                Some(if app.prefs.autoplay { "On" } else { "Off" }.into()),
                false,
                Message::ToggleAutoplay,
            )];
            if !st.is_live {
                v.push(item(
                    labelled(Icon::Speed, "Playback speed"),
                    Some(speed_label(st.speed)),
                    true,
                    Message::Menu(Some(MenuPage::Speed)),
                ));
            }
            let q = match quality {
                None if st.height > 0 => format!("Auto ({}p)", st.height),
                None => "Auto".into(),
                Some(h) => format!("{h}p"),
            };
            v.push(item(
                labelled(Icon::Tune, "Quality"),
                Some(q),
                true,
                Message::Menu(Some(MenuPage::Quality)),
            ));
            v
        }
        MenuPage::Speed => {
            let mut v = vec![header("Playback speed")];
            v.extend(SPEEDS.iter().map(|s| {
                check_row(
                    speed_label(*s),
                    (s - st.speed).abs() < 0.01,
                    Message::SetSpeed(*s),
                )
            }));
            v
        }
        MenuPage::Quality => {
            let mut v = vec![header("Quality")];
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
    let mh = (n * 40.0 + 16.0).min((ph - 72.0).max(80.0));
    let mw = 260.0_f32.min(pw - 24.0);
    let menu = container(
        iced::widget::scrollable(column(rows))
            .height(Length::Fixed(mh - 16.0))
            .width(Fill),
    )
    .style(t::menu)
    .padding(Padding {
        top: 8.0,
        bottom: 8.0,
        ..Padding::ZERO
    })
    .width(mw)
    .height(mh);
    pin(menu)
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
                button(txt("Cancel", t::BODY_M))
                    .padding(Padding {
                        top: 8.0,
                        bottom: 8.0,
                        left: 16.0,
                        right: 16.0
                    })
                    .style(t::pill)
                    .on_press(Message::UpNextCancel),
                button(txt("Play now", t::BODY_M))
                    .padding(Padding {
                        top: 8.0,
                        bottom: 8.0,
                        left: 16.0,
                        right: 16.0
                    })
                    .style(t::primary)
                    .on_press(Message::UpNextPlay),
                Space::new().width(Fill),
                button(container(icon(Icon::Replay, 24.0, t::TEXT)).center(Fill))
                    .width(36)
                    .height(36)
                    .padding(0)
                    .style(t::pill)
                    .on_press(Message::Replay),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12),
    )
    .style(t::overlay_card)
    .padding(16)
    .width(cw);
    container(card)
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
    let mut names = column![
        button(
            container(
                txt(name, t::CARD_TITLE)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .color(t::TEXT)
            )
            .max_width((width * 0.35).clamp(120.0, 260.0))
            .clip(true)
        )
        .padding(0)
        .style(t::bare)
        .on_press(Message::OpenWatchChannel)
    ];
    if let Some(s) = subs {
        names = names.push(txt(s, t::META_S).color(t::TEXT_2));
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
        button(
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
        .on_press(msg)
        .into()
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

pub fn description<'a>(info: &'a WatchInfo, w: &'a Watch) -> El<'a> {
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
        col = col
            .push(rich(segs, t::BODY, 0).color(t::TEXT).on_link(on_link))
            .push(Space::new().height(8))
            .push(
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
