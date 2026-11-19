//! The watch page: the player with its controls, the title and owner row, the description,
//! comments and related videos.

use crate::app::App;
use crate::pages::*;
use crate::theme::*;
use crate::video::{self, CLOCK_PLACEHOLDER, Menu, SPEEDS, speed_label};
use crate::view::{
    avatar, context_menu, focus, icon_button, more_button, pill, subscribe_button, thumb,
};
use nt_core::{Status, Video, library};
use rune::{
    Color, Cursor, El, Icon, Link, LinkPaint, Phase, Span, SurfaceFit, Tx, col, el, icon, rich,
    row, text,
};
use rune_kit::Spinner;

// ----- player -----------------------------------------------------------------

/// The player box, `w` × `h` points: the default view, theatre (`w` is the window width) or
/// fullscreen.
pub fn player(s: &App, w: f32, h: f32, fullscreen: bool) -> El {
    let tok = t();
    let v = &s.video;
    let sh = &v.shown;
    let edge = fullscreen || s.prefs.theatre;
    let radius = if edge { 0.0 } else { 12.0 };
    let failed = match (&v.error, &sh.status) {
        (Some(_), _) => Some("This video can't be played right now.".to_string()),
        (_, Status::Failed(e)) => {
            eprintln!("nt-rune: playback failed: {e}");
            Some("This video is unavailable.".to_string())
        }
        _ => None,
    };
    let mut p = el()
        .key("player")
        .w(w)
        .h(h)
        .radius(radius)
        .fill(Color::BLACK)
        .clip_rounded(radius)
        .cursor(if v.controls {
            Cursor::Default
        } else {
            Cursor::Hidden
        })
        .on_hover_change(|s: &mut App, on: bool, _: &mut Tx| s.player_hover(on))
        .on_pointer_move_local(|s: &mut App, _, tx: &mut Tx| s.player_moved(tx))
        .on_press(|s: &mut App| s.click_player())
        .on_double_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_fullscreen(tx));
    if sh.has_picture {
        p = p.child(
            el().key("picture")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .pixels_fit(video::pixels(), SurfaceFit::Fit),
        );
    }
    if let Some(msg) = failed {
        return p.child(
            col()
                .key("player-error")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .center()
                .gap(16.0)
                .px(24.0)
                .child(icon(&ERROR, 48.0).color(tok.text))
                .child(text(msg).font(S16).color(tok.text).wrap().max_lines(3))
                .child(
                    pill("Retry", None, false)
                        .key("retry-play")
                        .on_press(|s: &mut App| s.video.retry()),
                ),
        );
    }
    if sh.waiting() && v.up_next.is_none() {
        p = p.child(
            col()
                .key("player-spin")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .center()
                .child(Spinner::new().size(48.0).color(Color::WHITE).build(kit())),
        );
    }
    if let Some((play_icon, tween)) = &v.bezel {
        p = p.child(
            col().absolute(0.0, 0.0).w(w).h(h).center().inert().child(
                el().key("bezel")
                    .size(52.0, 52.0)
                    .radius(26.0)
                    .fill(Color::BLACK.alpha(0.5))
                    .center()
                    .live_opacity(tween)
                    .child(icon(if *play_icon { &PLAY } else { &PAUSE }, 36.0).color(Color::WHITE)),
            ),
        );
    }
    if v.up_next.is_some() {
        p = p.child(up_next(s, w, h));
    }
    if v.controls && sh.status != Status::Idle {
        p = p.child(controls(s, w, h, fullscreen));
    }
    p
}

fn controls(s: &App, w: f32, h: f32, fullscreen: bool) -> El {
    let tok = t();
    let v = &s.video;
    let sh = &v.shown;
    let ended = sh.status == Status::Ended;
    let (play_icon, play_label): (&'static Icon, &'static str) = if ended {
        (&REPLAY, "Replay")
    } else if sh.playing() {
        (&PAUSE, "Pause (k)")
    } else {
        (&PLAY, "Play (k)")
    };
    let white = Color::WHITE;
    let hot = v.scrub_hover || v.drag.is_some();
    let track_h = if hot { 5.0 } else { 3.0 };
    let mut track = el()
        .key("track")
        .w(v.seek_w)
        .h(track_h)
        .radius(track_h / 2.0)
        .fill(white.alpha(0.2))
        .clip_soft(0.0)
        .child(
            el().absolute(0.0, 0.0)
                .h(5.0)
                .fill(white.alpha(0.4))
                .live_rect(&v.buffered),
        )
        .child(
            el().absolute(0.0, 0.0)
                .h(5.0)
                .fill(tok.red)
                .live_rect(&v.progress),
        );
    if sh.is_live {
        track = track.fill(tok.red);
    }
    let mut scrub = el()
        .key("scrubber")
        .w(v.seek_w)
        .h(16.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .on_hover_change(|s: &mut App, on: bool, _: &mut Tx| s.video.scrub_hover = on)
        .on_pointer_move_local(|s: &mut App, e, tx: &mut Tx| {
            s.video.scrub_at(e.x);
            s.player_moved(tx);
        })
        .on_drag(
            |s: &mut App, d: &mut rune::Drag, _: &mut Tx| match d.phase {
                Phase::Down => {
                    s.video.drag = Some(0.0);
                    s.video.scrub_at(d.x);
                }
                Phase::Move => s.video.scrub_at(d.x),
                Phase::Up => {
                    // One seek, on release.
                    if let Some(f) = s.video.drag.take() {
                        s.video.seek_frac(f);
                    }
                }
            },
        )
        .child(track);
    if hot && !sh.is_live {
        scrub = scrub
            .child(
                el().key("knob")
                    .absolute(0.0, 1.5)
                    .radius(6.5)
                    .fill(tok.red)
                    .live_rect(&v.knob),
            )
            .child(
                el().key("tip")
                    .absolute(0.0, -34.0)
                    .w(48.0)
                    .h(24.0)
                    .radius(4.0)
                    .center()
                    .fill(Color::BLACK.alpha(0.7))
                    .live_offset(&v.tip)
                    .inert()
                    .child(
                        text("00:00:00")
                            .live_text(&v.tip_text)
                            .font(S12M)
                            .color(white),
                    ),
            );
    }
    let vol = if sh.muted {
        0.0
    } else {
        v.volume() as f32 / 100.0
    };
    let vol_icon: &'static Icon = if sh.muted || vol == 0.0 {
        &MUTED
    } else if vol < 0.5 {
        &VOLUME_LOW
    } else {
        &VOLUME
    };
    let mut volume = row()
        .key("volume-group")
        .h(40.0)
        .center_y()
        .on_hover_change(|s: &mut App, on: bool, _: &mut Tx| s.video.vol_hover = on)
        .child(
            icon_button("mute", vol_icon, "Mute (m)", white)
                .on_press(|s: &mut App| s.video.toggle_mute()),
        );
    if v.vol_hover {
        volume = volume.child(
            el().key("volume")
                .w(60.0)
                .h(40.0)
                .pr(8.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .on_drag(|s: &mut App, d: &mut rune::Drag, _: &mut Tx| {
                    let f = (d.x / 52.0).clamp(0.0, 1.0);
                    s.video.set_volume((f * 100.0) as f64);
                })
                .child(
                    el().w(52.0)
                        .h(3.0)
                        .radius(1.5)
                        .fill(white.alpha(0.2))
                        .child(el().absolute(0.0, 0.0).w(52.0 * vol).h(3.0).fill(white))
                        .child(
                            el().absolute(52.0 * vol - 6.0, -4.5)
                                .size(12.0, 12.0)
                                .radius(6.0)
                                .fill(white),
                        ),
                ),
        );
    }
    let left = row()
        .gap(4.0)
        .center_y()
        .child(
            icon_button("play", play_icon, play_label, white)
                .on_press(|s: &mut App| s.click_player()),
        )
        .child(volume)
        .child(if sh.is_live {
            row()
                .gap(8.0)
                .pl(8.0)
                .center_y()
                .child(el().size(8.0, 8.0).radius(4.0).fill(tok.red))
                .child(text("LIVE").font(S13M).color(white))
        } else {
            el().pl(8.0).child(
                text(CLOCK_PLACEHOLDER)
                    .live_text(&v.clock)
                    .font(S13_TNUM)
                    .color(white),
            )
        });
    let mut right = row().gap(4.0).center_y().child(
        icon_button("settings", &SETTINGS, "Settings", white).on_press(|s: &mut App| {
            s.video.menu = if s.video.menu == Menu::Closed {
                Menu::Main
            } else {
                Menu::Closed
            };
            s.refresh_controls();
        }),
    );
    if !fullscreen {
        right = right.child(
            icon_button("theatre", &THEATRE, "Theatre mode (t)", white)
                .on_press(|s: &mut App| s.toggle_theatre()),
        );
    }
    right = right.child(
        icon_button(
            "fullscreen",
            if fullscreen {
                &FULLSCREEN_EXIT
            } else {
                &FULLSCREEN
            },
            "Full screen (f)",
            white,
        )
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_fullscreen(tx)),
    );
    col()
        .key("controls")
        .absolute(0.0, (h - 120.0).max(0.0))
        .w(w)
        .h(120.0f32.min(h))
        .px(12.0)
        .justify_end()
        .linear_gradient(
            std::f32::consts::PI,
            [(0.0, Color::TRANSPARENT), (1.0, Color::BLACK.alpha(0.6))],
        )
        .child(scrub)
        .child(
            row()
                .w_full()
                .h(48.0)
                .center_y()
                .justify_between()
                .child(left)
                .child(right),
        )
}

/// The settings menu, as a window-level layer over the player's bottom-right corner (the player
/// box `[x, y, w, h]` in window points). A full-window catcher closes it on a press elsewhere
/// without that press reaching anything underneath.
pub fn settings_layer(s: &App, r: [f32; 4]) -> El {
    let [x, y, w, h] = r;
    let menu = settings_menu(s, w, h);
    el().key("settings-layer")
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .on_press(|s: &mut App| {
            s.video.menu = Menu::Closed;
            s.refresh_controls();
        })
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.video.menu = Menu::Closed)
        .child(el().absolute(x, y).w(w).h(h).child(menu))
}

fn settings_menu(s: &App, w: f32, h: f32) -> El {
    let tok = t();
    let v = &s.video;
    let sh = &v.shown;
    let line = |key: &str, label: String, value: Option<String>, check: bool| {
        let mut r = row()
            .key(key.to_string())
            .w_full()
            .h(40.0)
            .px(16.0)
            .gap(12.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .hover(tok.fill);
        r = r.child(el().w(24.0).child(if check {
            icon(&CHECK, 20.0).color(tok.text)
        } else {
            el()
        }));
        r = r.child(text(label).font(S14).color(tok.text).grow(1.0));
        if let Some(val) = value {
            r = r
                .child(text(val).font(S14).color(tok.text2))
                .child(icon(&CHEVRON_RIGHT, 20.0).color(tok.text2));
        }
        r
    };
    let header = |label: &'static str| {
        row()
            .key(format!("menu-back-{label}"))
            .w_full()
            .h(48.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .border_sides([0.0, 0.0, 0.0, 1.0], tok.divider)
            .on_press(|s: &mut App| s.video.menu = Menu::Main)
            .child(icon(&CHEVRON_LEFT, 24.0).color(tok.text))
            .child(text(label).font(S14M).color(tok.text))
    };
    let auto_label = if sh.height > 0 {
        format!("Auto ({}p)", sh.height)
    } else {
        "Auto".to_string()
    };
    let mut m = col()
        .key("settings-menu")
        .w(280.0)
        .py(8.0)
        .radius(12.0)
        .fill(tok.menu.alpha(0.94))
        .on_press(|_: &mut App| {});
    match v.menu {
        Menu::Main | Menu::Closed => {
            let autoplay = s.prefs.autoplay;
            m = m.child(
                line("set-autoplay", "Autoplay".into(), None, autoplay)
                    .on_press(|s: &mut App| s.toggle_autoplay()),
            );
            if !sh.is_live {
                m = m.child(
                    line(
                        "set-speed",
                        "Playback speed".into(),
                        Some(speed_label(sh.speed())),
                        false,
                    )
                    .on_press(|s: &mut App| s.video.menu = Menu::Speed),
                );
            }
            let q = match sh.quality {
                Some(h) => format!("{h}p"),
                None => auto_label.clone(),
            };
            m = m.child(
                line("set-quality", "Quality".into(), Some(q), false)
                    .on_press(|s: &mut App| s.video.menu = Menu::Quality),
            );
        }
        Menu::Speed => {
            m = m.child(header("Playback speed"));
            for sp in SPEEDS {
                let cur = (sh.speed() - sp).abs() < 0.01;
                m = m.child(
                    line(&format!("speed-{sp}"), speed_label(sp), None, cur).on_press(
                        move |s: &mut App| {
                            s.video.set_speed(sp);
                            s.video.menu = Menu::Closed;
                            s.refresh_controls();
                        },
                    ),
                );
            }
        }
        Menu::Quality => {
            m = m.child(header("Quality"));
            let mut heights = sh.heights.clone();
            heights.sort_unstable_by(|a, b| b.cmp(a));
            heights.dedup();
            for h in heights {
                m = m.child(
                    line(
                        &format!("q-{h}"),
                        format!("{h}p"),
                        None,
                        sh.quality == Some(h),
                    )
                    .on_press(move |s: &mut App| {
                        s.video.set_quality(Some(h));
                        s.video.menu = Menu::Closed;
                        s.refresh_controls();
                    }),
                );
            }
            m = m.child(
                line("q-auto", auto_label, None, sh.quality.is_none()).on_press(|s: &mut App| {
                    s.video.set_quality(None);
                    s.video.menu = Menu::Closed;
                    s.refresh_controls();
                }),
            );
        }
    }
    let rows = match v.menu {
        Menu::Speed => SPEEDS.len() as f32 * 40.0 + 64.0,
        Menu::Quality => (sh.heights.len() as f32 + 1.0) * 40.0 + 64.0,
        _ => 3.0 * 40.0 + 16.0,
    };
    let top = (h - 64.0 - rows).max(8.0);
    m.absolute((w - 292.0).max(8.0), top)
}

fn up_next(s: &App, w: f32, h: f32) -> El {
    let tok = t();
    let next = s.top().feed.items.first();
    let mut c = col()
        .key("up-next")
        .absolute(0.0, 0.0)
        .w(w)
        .h(h)
        .center()
        .gap(16.0)
        .fill(Color::BLACK.alpha(0.75))
        .on_press(|_: &mut App| {})
        .child(
            text("Up next in 5")
                .live_text(&s.video.up_next_text)
                .font(S14)
                .color(tok.text2),
        );
    if let Some(v) = next {
        let tw = (w * 0.3).clamp(160.0, 320.0);
        c = c
            .child(thumb(s, v, tw, tw * 9.0 / 16.0, 8.0, s.scale.get()))
            .child(
                text(v.title.clone())
                    .font(S16M)
                    .color(tok.text)
                    .max_lines(2)
                    .max_w((w - 48.0).max(100.0)),
            )
            .child(text(v.channel.clone()).font(S14).color(tok.text2));
    }
    c.child(
        row()
            .gap(12.0)
            .child(
                pill("Cancel", None, false)
                    .key("up-next-cancel")
                    .on_press(|s: &mut App| s.cancel_up_next()),
            )
            .child(
                pill("Play now", Some(&NEXT), true)
                    .key("up-next-play")
                    .on_press_tx(|s: &mut App, tx: &mut Tx| {
                        s.video.up_next = Some(0.0);
                        tx.wake_after(std::time::Duration::ZERO);
                    }),
            ),
    )
}

// ----- below the player -----------------------------------------------------------

pub fn info(s: &App, page: &Page, b: &Block, scale: f32) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let title = w
        .info
        .as_ref()
        .map(|i| i.title.clone())
        .unwrap_or_else(|| w.video.title.clone());
    let channel = w.channel();
    let id = channel.id.clone();
    let open = move |s: &mut App, tx: &mut Tx| s.open_channel(id.clone(), tx);
    let mut owner = row()
        .h(40.0)
        .gap(12.0)
        .center_y()
        .min_w(0.0)
        .child(
            avatar(s, channel.avatar.as_deref(), 40.0, scale)
                .cursor(Cursor::Pointer)
                .on_press_tx(open.clone()),
        )
        .child(
            col()
                .min_w(0.0)
                .shrink()
                .cursor(Cursor::Pointer)
                .on_press_tx(open)
                .child(
                    text(channel.name.clone())
                        .font(S16M)
                        .color(tok.text)
                        .truncate(),
                )
                .child(
                    text(channel.subscribers.clone().unwrap_or_default())
                        .font(S12)
                        .color(tok.text2)
                        .truncate(),
                ),
        );
    if !channel.id.is_empty() {
        owner = owner.child(el().w(12.0)).child(subscribe_button(channel));
    }
    let video = w.as_video();
    let liked = w.rating;
    let rate = |up: bool| {
        move |s: &mut App| {
            let p = s.top_mut();
            if let Some(w) = p.watch_mut() {
                w.rating = if w.rating == Some(up) { None } else { Some(up) };
            }
            p.touch();
        }
    };
    let likes = w.info.as_ref().and_then(|i| i.likes.clone());
    let mut like = row()
        .key("like")
        .h(36.0)
        .pl(12.0)
        .pr(16.0)
        .gap(6.0)
        .center_y()
        .role("button")
        .label("Like")
        .cursor(Cursor::Pointer)
        .hover(tok.fill)
        .corners([18.0, 0.0, 0.0, 18.0])
        .on_press(rate(true))
        .child(
            icon(
                if liked == Some(true) {
                    &LIKE_FILL
                } else {
                    &LIKE
                },
                24.0,
            )
            .color(tok.text),
        );
    if let Some(n) = likes {
        like = like.child(text(n).font(S14M).color(tok.text));
    }
    let dislike = el()
        .key("dislike")
        .h(36.0)
        .px(12.0)
        .center()
        .role("button")
        .label("Dislike")
        .cursor(Cursor::Pointer)
        .hover(tok.fill)
        .corners([0.0, 18.0, 18.0, 0.0])
        .on_press(rate(false))
        .child(
            icon(
                if liked == Some(false) {
                    &DISLIKE_FILL
                } else {
                    &DISLIKE
                },
                24.0,
            )
            .color(tok.text),
        );
    let rating = row()
        .h(36.0)
        .radius(18.0)
        .fill(tok.fill)
        .center_y()
        .child(focus(like))
        .child(el().w(1.0).h(24.0).fill(tok.divider))
        .child(focus(dislike));
    let vid = video.id.clone();
    let saved = library().is_saved(&video.id);
    let actions = row()
        .gap(8.0)
        .center_y()
        .child(rating)
        .child(
            pill("Share", Some(&SHARE), false)
                .key("share")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.share(&vid, tx)),
        )
        .child(
            pill(
                if saved { "Saved" } else { "Watch Later" },
                Some(if saved { &LATER_FILL } else { &LATER }),
                false,
            )
            .key("later")
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_later(&video, tx)),
        );
    let mut c = col()
        .key("watch-info")
        .w_full()
        .gap(12.0)
        .child(text(title).font(S20B).color(tok.text).wrap());
    if b.w < OWNER_WRAP {
        c = c.child(owner).child(actions);
    } else {
        c = c.child(
            row()
                .w_full()
                .h(40.0)
                .center_y()
                .justify_between()
                .child(owner)
                .child(actions),
        );
    }
    c
}

pub fn skel_info(w: f32) -> El {
    let tok = t();
    let bar = |w: f32, h: f32| el().w(w).h(h).radius(4.0).fill(tok.skeleton);
    col()
        .key("skel-info")
        .w_full()
        .gap(12.0)
        .child(bar(w * 0.9, 20.0))
        .child(bar(w * 0.6, 14.0))
        .child(bar(w * 0.7, 14.0))
}

/// Links in a description: URLs, #tags and timestamps, as (range, id) with ids `url:…`,
/// `tag:…` and `t:<seconds>`.
fn find_links(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = text.get(i..).unwrap_or("");
        let at_word = i == 0
            || bytes
                .get(i - 1)
                .is_some_and(|b| b.is_ascii_whitespace() || *b == b'(');
        if at_word && (rest.starts_with("https://") || rest.starts_with("http://")) {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == ')')
                .unwrap_or(rest.len());
            out.push((i..i + end, format!("url:{}", rest.get(..end).unwrap_or(""))));
            i += end.max(1);
            continue;
        }
        if at_word && rest.starts_with('#') {
            let end = rest
                .char_indices()
                .skip(1)
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map_or(rest.len(), |(j, _)| j);
            if end > 1 {
                out.push((i..i + end, format!("tag:{}", rest.get(..end).unwrap_or(""))));
                i += end;
                continue;
            }
        }
        if at_word && bytes.get(i).is_some_and(u8::is_ascii_digit) {
            // h:mm:ss or m:ss
            let end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == ':'))
                .unwrap_or(rest.len());
            let tok = rest.get(..end).unwrap_or("");
            let parts: Vec<&str> = tok.split(':').collect();
            let ok = (2..=3).contains(&parts.len())
                && parts.iter().skip(1).all(|p| p.len() == 2)
                && parts.iter().all(|p| !p.is_empty());
            if ok {
                let secs = parts
                    .iter()
                    .fold(0u64, |acc, p| acc * 60 + p.parse::<u64>().unwrap_or(0));
                out.push((i..i + end, format!("t:{secs}")));
                i += end;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

fn linked(content: String, style: rune::TextStyle, extra: Vec<Span>) -> El {
    let tok = t();
    let links = find_links(&content);
    let mut spans: Vec<Span> = links
        .iter()
        .map(|(r, _)| Span::new(r.start, r.end).color(tok.link))
        .collect();
    spans.extend(extra);
    let mut e = rich(content, spans).font(style).color(tok.text).wrap();
    for (r, id) in links {
        e = e.link(Link::new(r, id).hover(LinkPaint::new().underline(tok.link)));
    }
    e.on_link(|s: &mut App, id: &str, tx: &mut Tx| {
        if let Some(url) = id.strip_prefix("url:") {
            let _ = tx.open_url(url);
        } else if let Some(tag) = id.strip_prefix("tag:") {
            s.search.replace_text(tag.to_string());
            s.submit_search(tx);
        } else if let Some(t) = id.strip_prefix("t:").and_then(|t| t.parse::<f64>().ok()) {
            s.video.seek(t);
            s.top_mut()
                .scroll
                .anim
                .scroll_to(0.0, crate::scroller::now());
            tx.wake_after(std::time::Duration::ZERO);
        }
    })
}

pub fn description(page: &Page, plan: &Plan, b: &Block) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let Some(info) = &w.info else { return el() };
    let head = [info.views.clone(), info.date.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ");
    let toggle = |s: &mut App| {
        let p = s.top_mut();
        if let Some(w) = p.watch_mut() {
            w.desc_open = !w.desc_open;
        }
        p.touch();
    };
    let mut c = col()
        .key("desc")
        .w_full()
        .h_full()
        .padding(12.0)
        .radius(12.0)
        .fill(tok.fill)
        .gap(4.0)
        .child(text(head).font(S14M).color(tok.text));
    if info.description.is_empty() {
        return c;
    }
    if w.desc_open {
        c = c
            .child(linked(info.description.clone(), S14, Vec::new()))
            .child(
                text("Show less")
                    .font(S14M)
                    .color(tok.text)
                    .mt(8.0)
                    .cursor(Cursor::Pointer)
                    .on_press(toggle),
            );
    } else if plan.desc_more {
        let cut = description_cut(w, b.w - 24.0).unwrap_or(info.description.len());
        let shown = info
            .description
            .get(..cut)
            .unwrap_or("")
            .trim_end()
            .to_string();
        let start = shown.len();
        let content = format!("{shown}…more");
        let more = Span::new(start, content.len()).weight(500.0);
        c = c
            .hover(tok.fill)
            .cursor(Cursor::Pointer)
            .on_press(toggle)
            .child(linked(content, S14, vec![more]).max_lines(3));
    } else {
        c = c.child(linked(info.description.clone(), S14, Vec::new()));
    }
    c
}

pub fn comments_head(page: &Page) -> El {
    let tok = t();
    let Some(w) = page.watch() else { return el() };
    let label = match &w.comments_total {
        Some(n) => format!("{n} Comments"),
        None => "Comments".to_string(),
    };
    row()
        .key("comments-head")
        .w_full()
        .h_full()
        .center_y()
        .child(text(label).font(S20B).color(tok.text))
}

pub fn comment(s: &App, page: &Page, c: &nt_core::Comment, i: usize, b: &Block, scale: f32) -> El {
    let tok = t();
    let open = page
        .watch()
        .is_some_and(|w| w.comments_open.contains(&c.id));
    let full = measure(&c.text, S14, comment_text_w(b.w));
    let clamped = full > 80.5;
    let mut head = row().gap(4.0).center_y().child(
        text(c.author.clone())
            .font(S13M)
            .color(tok.text)
            .truncate()
            .shrink(),
    );
    if let Some(p) = &c.published {
        head = head.child(text(p.clone()).font(S12).color(tok.text2));
    }
    let mut body = text(c.text.clone()).font(S14).color(tok.text).wrap();
    if !open {
        body = body.max_lines(4);
    }
    let mut col_ = col().grow(1.0).min_w(0.0).gap(2.0).child(head).child(body);
    if clamped {
        let id = c.id.clone();
        col_ = col_.child(
            text(if open { "Show less" } else { "Read more" })
                .font(S14M)
                .color(tok.text2)
                .mt(4.0)
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| {
                    let p = s.top_mut();
                    if let Some(w) = p.watch_mut()
                        && !w.comments_open.remove(&id)
                    {
                        w.comments_open.insert(id.clone());
                    }
                    p.touch();
                }),
        );
    }
    let mut foot = row()
        .h(32.0)
        .mt(8.0)
        .gap(8.0)
        .center_y()
        .child(icon(&LIKE, 16.0).color(tok.text2));
    if let Some(l) = c.likes.as_ref().filter(|l| !l.is_empty()) {
        foot = foot.child(text(l.clone()).font(S12).color(tok.text2));
    }
    foot = foot
        .child(el().w(8.0))
        .child(icon(&DISLIKE, 16.0).color(tok.text2));
    row()
        .key(format!("cm-{i}-{}", c.id))
        .w_full()
        .gap(16.0)
        .child(avatar(s, c.author_avatar.as_deref(), 40.0, scale))
        .child(col_.child(foot))
}

pub fn compact(s: &App, v: &Video, i: usize, b: &Block, scale: f32) -> El {
    let tok = t();
    let open = v.clone();
    let go = move |s: &mut App, tx: &mut Tx| {
        if s.press_ok(tx) {
            s.open_video(open.clone(), tx);
        }
    };
    // The whole row is one link: it takes the press, the focus and the right-click; nothing
    // inside it is a separate press target, so a click anywhere on it opens the video.
    focus(
        row()
            .key(format!("rel-{i}-{}", v.id))
            .w(b.w)
            .h(COMPACT_H)
            .gap(8.0)
            .role("link")
            .label(v.title.clone())
            .hover_group()
            .cursor(Cursor::Pointer)
            .press(Color::BLACK.alpha(0.1))
            .on_press_tx(go)
            .on_secondary_press(context_menu(v))
            .child(thumb(s, v, 168.0, 94.0, 8.0, scale))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(
                        text(v.title.clone())
                            .font(S14M)
                            .color(tok.text)
                            .max_lines(2),
                    )
                    .child(
                        text(v.channel.clone())
                            .font(S12)
                            .color(tok.text2)
                            .truncate(),
                    )
                    .child(text(v.meta_line()).font(S12).color(tok.text2).truncate()),
            )
            .child(more_button(v, [b.x + b.w - 32.0, b.y]).mr(-8.0)),
    )
}

pub fn skel_compact(i: usize) -> El {
    let tok = t();
    let bar = |w: f32| el().w(w).h(14.0).radius(4.0).fill(tok.skeleton);
    row()
        .key(format!("skel-rel-{i}"))
        .w_full()
        .h(COMPACT_H)
        .gap(8.0)
        .child(el().w(168.0).h(94.0).radius(8.0).fill(tok.skeleton))
        .child(col().grow(1.0).gap(8.0).child(bar(160.0)).child(bar(100.0)))
}
