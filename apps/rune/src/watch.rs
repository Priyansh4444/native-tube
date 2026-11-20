//! The watch page: the player with its controls, the title and owner row, the description,
//! comments and related videos.

use crate::app::App;
use crate::pages::*;
use crate::probe::target;
use crate::theme::*;
use crate::video::{CLOCK_PLACEHOLDER, Menu, SLEEP_MINUTES, SPEEDS, Sleep, speed_label};
use crate::view::{
    avatar, badges, context_menu, focus, icon_button, more_button, pill, subscribe_button, thumb,
    verified_name,
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
    let hardware = s.native_picture();
    let edge = fullscreen || s.prefs.theatre;
    let radius = if edge { 0.0 } else { 12.0 };
    let failed = match (&v.error, &sh.status) {
        (Some(_), _) => Some("This video can't be played right now.".to_string()),
        (_, Status::Failed(_)) => Some("This video is unavailable.".to_string()),
        _ => None,
    };
    let mut p = el()
        .key("player")
        .w(w)
        .h(h)
        .radius(radius)
        // Never animate the player's own fill: Rune springs colour changes even in instant
        // commits, so a black→transparent fill faded over the GPU video's hole for a moment
        // after each switch (a near-black first frame). The black backing is its own layer,
        // mounted only for software pictures and removed at once.
        .fill(Color::TRANSPARENT)
        .clip_rounded(radius)
        .cursor(if v.controls {
            Cursor::Default
        } else {
            Cursor::Hidden
        })
        .on_hover_change(|s: &mut App, on: bool, tx: &mut Tx| s.player_hover(on, tx))
        .on_pointer_move_local(|s: &mut App, _, tx: &mut Tx| s.player_moved(tx))
        .on_press(|s: &mut App| {
            if s.player_menu.take().is_none() {
                s.click_player();
            }
        })
        .on_double_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_fullscreen(tx))
        .on_secondary_press(|s: &mut App, p: rune::PointerPress, _: &mut Tx| {
            s.video.menu = Menu::Closed;
            s.card_menu = None;
            s.player_menu = Some([p.x, p.y]);
        });
    if !hardware {
        p = p.child(
            el().key("player-backing")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .fill(Color::BLACK)
                .transition(rune::Transition::none())
                .inert(),
        );
    }
    if !hardware && let Some(frame) = &v.poster_frame {
        p = p.child(
            el().key("previous-picture")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .pixels_fit(frame, SurfaceFit::Fit)
                .inert(),
        );
    } else if !hardware && !sh.has_picture && failed.is_none() {
        // Until this video's first picture: its poster (never the previous video's frame).
        if let Some(url) = s.top().watch().map(|w| w.video.thumbnail.clone())
            && !url.is_empty()
        {
            p = p.child(
                el().key("poster")
                    .absolute(0.0, 0.0)
                    .w(w)
                    .h(h)
                    .pixels_fit(
                        &s.images.get(&url, crate::view::px(w, s.scale.get())),
                        SurfaceFit::Fit,
                    )
                    .inert(),
            );
        }
    }
    if !hardware && sh.has_picture {
        p = p.child(
            el().key("picture")
                .absolute(0.0, 0.0)
                .w(w)
                .h(h)
                .pixels_fit(&v.picture, SurfaceFit::Fit)
                .inert(),
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
    if !hardware && (sh.waiting() || s.gpu_pending()) && v.up_next.is_none() {
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
    if v.stats_open {
        p = p.child(stats_overlay(s, w));
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
    let sw = v.seek_w;
    // One segment per chapter, 2 px apart (one segment without chapters). Each clips the same
    // live fill, moved by the segment's own offset, so playback still runs no view.
    let mut cuts: Vec<f32> = Vec::new();
    if v.duration > 0.0 && v.chapters.len() > 1 && !sh.is_live {
        for (start, _) in v.chapters.iter().skip(1) {
            let x = (*start / v.duration).clamp(0.0, 1.0) as f32 * sw;
            if x > 2.0 && x < sw - 2.0 {
                cuts.push(x.round());
            }
        }
    }
    let mut track = el().key("track").w(sw).h(track_h);
    let mut from = 0.0f32;
    for (i, end) in cuts.iter().copied().chain(std::iter::once(sw)).enumerate() {
        let x0 = if i == 0 { 0.0 } else { from + 1.0 };
        let x1 = if end >= sw { sw } else { end - 1.0 };
        from = end;
        if x1 - x0 < 0.5 {
            continue;
        }
        let fill = el()
            .absolute(-x0, 0.0)
            .w(sw)
            .h(5.0)
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
        track = track.child(
            el().key(format!("seg-{i}"))
                .absolute(x0, 0.0)
                .w(x1 - x0)
                .h(track_h)
                .radius(if cuts.is_empty() { track_h / 2.0 } else { 0.0 })
                .fill(if sh.is_live {
                    tok.red
                } else {
                    white.alpha(0.2)
                })
                .clip_soft(0.0)
                .child(fill),
        );
    }
    let mut scrub = el()
        .key("scrubber")
        .w(v.seek_w)
        .h(16.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .on_hover_change(|s: &mut App, on: bool, tx: &mut Tx| {
            s.video.scrub_hover = on;
            if !on && s.video.drag.is_none() {
                s.video.clear_preview();
            }
            tx.instant();
        })
        .on_pointer_move_local(|s: &mut App, e, tx: &mut Tx| {
            s.video.scrub_at(e.x, s.scale.get());
            s.player_moved(tx);
        })
        .on_drag(
            |s: &mut App, d: &mut rune::Drag, _: &mut Tx| match d.phase {
                Phase::Down => {
                    s.video.drag = Some(0.0);
                    s.video.scrub_at(d.x, s.scale.get());
                }
                Phase::Move => s.video.scrub_at(d.x, s.scale.get()),
                Phase::Up => {
                    // One seek, on release.
                    if let Some(f) = s.video.drag.take() {
                        s.video.seek_frac(f);
                        if !s.video.scrub_hover {
                            s.video.clear_preview();
                        }
                    }
                }
            },
        )
        .child(track);
    if hot && !sh.is_live && !v.heat.is_empty() && v.duration > 0.0 {
        scrub = scrub.child(heatmap(&v.heat, v.duration, sw));
    }
    if hot && !sh.is_live {
        scrub = scrub
            .child(
                el().key("knob")
                    .absolute(0.0, 1.5)
                    .radius(6.5)
                    .fill(tok.red)
                    .live_rect(&v.knob),
            )
            .child(scrub_tip(s));
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
    let mut left = row().gap(4.0).center_y().child(
        icon_button("play", play_icon, play_label, white).on_press(|s: &mut App| s.click_player()),
    );
    if let Some(index) = s
        .top()
        .watch()
        .and_then(|w| w.playlist.as_ref())
        .and_then(|p| p.index)
        .and_then(|i| i.checked_sub(1))
    {
        left = left.child(
            icon_button("previous", &CHEVRON_LEFT, "Previous video", white)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.play_playlist_item(index, tx)),
        );
    }
    if s.next_video().is_some() {
        left = left.child(
            icon_button("next", &NEXT, "Next video", white)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.play_next(tx)),
        );
    }
    let left = left.child(volume).child(if sh.is_live {
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
    let left = if v.chapters.is_empty() || sh.is_live {
        left
    } else {
        // "• Intro ›": the chapter under the playhead (a live text: no view run per chapter).
        // A live text paints inside the box it is given (its placeholder's, by default).
        left.child(
            el().key("chapter-title").pl(4.0).child(
                text("")
                    .live_text(&v.chapter_text)
                    .font(S13M)
                    .color(white)
                    .truncate_fade()
                    .size((w * 0.3).clamp(80.0, 320.0), 18.0),
            ),
        )
    };
    let mut right = row().gap(4.0).center_y();
    if sh.cc_count > 0 {
        let on = sh.cc_sel.is_some();
        right = right.child(
            col()
                .key("cc-wrap")
                .items_center()
                .child(
                    icon_button(
                        "cc",
                        if on { &CC_FILL } else { &CC },
                        "Subtitles/closed captions (c)",
                        white,
                    )
                    .on_press(|s: &mut App| s.video.toggle_captions()),
                )
                .child(
                    el().w(if on { 20.0 } else { 0.0 })
                        .h(3.0)
                        .mt(-6.0)
                        .radius(1.5)
                        .fill(tok.red),
                ),
        );
    }
    right = right.child(
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
        .instant_changes()
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

fn scrub_tip(s: &App) -> El {
    let v = &s.video;
    let preview = v.has_storyboard();
    let mut tip = col()
        .key("tip")
        .absolute(0.0, if preview { -150.0 } else { -58.0 })
        .w(160.0)
        .items_center()
        .gap(4.0)
        .py(4.0)
        .radius(6.0)
        .fill(Color::BLACK.alpha(0.85))
        .live_offset(&v.tip)
        .inert();
    if preview {
        tip = tip.child(
            el().size(160.0, 90.0)
                .pixels_fit(&v.preview, SurfaceFit::Fit),
        );
    }
    tip = tip.child(
        text("0:00")
            .live_text(&v.tip_text)
            .font(S12M)
            .color(Color::WHITE)
            .align(rune::Align::Center)
            .size(150.0, 18.0),
    );
    if !v.chapters.is_empty() {
        tip = tip.child(
            text("")
                .live_text(&v.tip_chapter)
                .font(S12)
                .color(Color::WHITE)
                .truncate_fade()
                .align(rune::Align::Center)
                .size(150.0, 18.0),
        );
    }
    tip
}

/// The replay heatmap over the scrubber: the samples' spans as an area, normalised to 0–1.
fn heatmap(heat: &[nt_core::HeatPoint], duration: f64, w: f32) -> El {
    const H: f32 = 40.0;
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(heat.len() * 2 + 2);
    pts.push([0.0, H]);
    for p in heat {
        let x0 = (p.start / duration).clamp(0.0, 1.0) as f32 * w;
        let x1 = ((p.start + p.duration) / duration).clamp(0.0, 1.0) as f32 * w;
        let y = H - (p.intensity.clamp(0.0, 1.0) as f32) * H;
        pts.push([(x0 + x1) / 2.0, y]);
    }
    pts.push([w, H]);
    el().key("heatmap")
        .absolute(0.0, -H + 4.0)
        .w(w)
        .h(H)
        .inert()
        .shape(rune::Shape::new().fill(pts, Color::WHITE.alpha(0.25)))
}

/// The settings menu, as a window-level layer over the player's bottom-right corner (the player
/// box `[x, y, w, h]` in window points). A full-window catcher closes it on a press elsewhere
/// without that press reaching anything underneath.
pub fn settings_layer(s: &App, r: [f32; 4]) -> El {
    let [x, y, w, h] = r;
    let menu = settings_menu(s, w, h);
    el().key("settings-layer")
        .transition(rune::Transition::none())
        .instant_changes()
        .modal_tx(false, |s: &mut App, tx| {
            s.video.menu = Menu::Closed;
            s.refresh_controls();
            tx.instant();
        })
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
        let mut r = focus(
            row()
                .key(key.to_string())
                .w_full()
                .h(40.0)
                .px(16.0)
                .gap(12.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .hover(tok.fill)
                .role("menuitem")
                .label(label.clone()),
        );
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
            .role("menuitem")
            .label(format!("Back to settings from {label}"))
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
            if sh.cc_count > 0 {
                m = m.child(
                    line(
                        "set-captions",
                        "Subtitles/CC".into(),
                        Some(caption_label(s)),
                        false,
                    )
                    .on_press(|s: &mut App| s.video.menu = Menu::Captions),
                );
            }
            if !v.audio_tracks.is_empty() {
                let label = v
                    .audio_tracks
                    .iter()
                    .find(|t| Some(&t.id) == sh.audio.as_ref())
                    .map(|t| t.label.clone())
                    .unwrap_or_default();
                m = m.child(
                    line("set-audio", "Audio track".into(), Some(label), false)
                        .on_press(|s: &mut App| s.video.menu = Menu::Audio),
                );
            }
            if sh.normalization_supported {
                m = m.child(
                    line("set-normalize", "Stable volume".into(), None, sh.normalize)
                        .on_press(|s: &mut App| s.video.toggle_normalize()),
                );
            }
            m = m.child(
                line("set-loop", "Loop".into(), None, sh.looping)
                    .on_press(|s: &mut App| s.video.toggle_loop()),
            );
            m = m.child(
                line(
                    "set-sleep",
                    "Sleep timer".into(),
                    Some(sleep_label(v.sleep)),
                    false,
                )
                .on_press(|s: &mut App| s.video.menu = Menu::Sleep),
            );
        }
        Menu::Audio => {
            m = m.child(header("Audio track"));
            for t in &v.audio_tracks {
                let id = t.id.clone();
                let label = if t.auto_dubbed {
                    format!("{} (auto-dubbed)", t.label)
                } else {
                    t.label.clone()
                };
                m = m.child(
                    line(
                        &format!("audio-{id}"),
                        label,
                        None,
                        sh.audio.as_ref() == Some(&id),
                    )
                    .on_press(move |s: &mut App| {
                        s.video.audio_track(id.clone());
                        s.video.menu = Menu::Closed;
                        s.refresh_controls();
                    }),
                );
            }
        }
        Menu::Captions => {
            m = m.child(header("Subtitles/CC"));
            m = m.child(
                line("cc-off", "Off".into(), None, sh.cc_sel.is_none()).on_press(|s: &mut App| {
                    s.video.set_captions(None);
                    s.video.menu = Menu::Closed;
                    s.refresh_controls();
                }),
            );
            for (i, c) in v.captions.iter().enumerate() {
                let label = if c.auto_generated {
                    format!("{} (auto-generated)", c.label)
                } else {
                    c.label.clone()
                };
                let on = sh
                    .cc_sel
                    .as_ref()
                    .is_some_and(|(j, l)| *j == i && l.is_none());
                m = m.child(line(&format!("cc-{i}"), label, None, on).on_press(
                    move |s: &mut App| {
                        s.video.set_captions(Some((i, None)));
                        s.video.menu = Menu::Closed;
                        s.refresh_controls();
                    },
                ));
            }
            // Auto-translate from the shown track, else the first one that allows it.
            let base = sh
                .cc_sel
                .as_ref()
                .map(|(i, _)| *i)
                .filter(|i| v.captions.get(*i).is_some_and(|c| c.translatable))
                .or_else(|| v.captions.iter().position(|c| c.translatable));
            if let Some(b) = base.filter(|_| !v.caption_languages.is_empty()) {
                let cur = sh
                    .cc_sel
                    .as_ref()
                    .and_then(|(_, l)| l.as_ref())
                    .and_then(|code| v.caption_languages.iter().find(|l| &l.code == code))
                    .map(|l| l.label.clone());
                m = m.child(
                    line(
                        "cc-translate",
                        "Auto-translate".into(),
                        Some(cur.unwrap_or_default()),
                        false,
                    )
                    .on_press(move |s: &mut App| s.video.menu = Menu::Translate(b)),
                );
            }
        }
        Menu::Translate(b) => {
            m = m.child(header("Auto-translate"));
            let mut list = col()
                .key("translate-list")
                .w_full()
                .h(320.0f32.min(h - 140.0).max(120.0))
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                });
            for (j, lang) in v.caption_languages.iter().enumerate() {
                let code = lang.code.clone();
                let on = sh
                    .cc_sel
                    .as_ref()
                    .is_some_and(|(i, l)| *i == b && l.as_deref() == Some(code.as_str()));
                list = list.child(
                    line(&format!("tl-{j}"), lang.label.clone(), None, on).on_press(
                        move |s: &mut App| {
                            s.video.set_captions(Some((b, Some(code.clone()))));
                            s.video.menu = Menu::Closed;
                            s.refresh_controls();
                        },
                    ),
                );
            }
            m = m.child(list);
        }
        Menu::Sleep => {
            m = m.child(header("Sleep timer"));
            m = m.child(
                line("sleep-off", "Off".into(), None, v.sleep == Sleep::Off).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.set_sleep(Sleep::Off, tx);
                        s.video.menu = Menu::Closed;
                        s.refresh_controls();
                    },
                ),
            );
            for min in SLEEP_MINUTES {
                let on = matches!(v.sleep, Sleep::At(_, m) if m == min);
                m = m.child(
                    line(&format!("sleep-{min}"), format!("{min} minutes"), None, on).on_press_tx(
                        move |s: &mut App, tx: &mut Tx| {
                            let due = crate::scroller::now() + min as f64 * 60.0;
                            s.set_sleep(Sleep::At(due, min), tx);
                            s.video.menu = Menu::Closed;
                            s.refresh_controls();
                        },
                    ),
                );
            }
            m = m.child(
                line(
                    "sleep-end",
                    "End of video".into(),
                    None,
                    v.sleep == Sleep::EndOfVideo,
                )
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.set_sleep(Sleep::EndOfVideo, tx);
                    s.video.menu = Menu::Closed;
                    s.refresh_controls();
                }),
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
    let rows = settings_h(s, h);
    let top = (h - 64.0 - rows).max(8.0);
    m.absolute((w - 292.0).max(8.0), top)
}

/// The settings menu's height in a player `h` tall.
pub fn settings_h(s: &App, h: f32) -> f32 {
    let v = &s.video;
    let sh = &v.shown;
    match v.menu {
        Menu::Speed => SPEEDS.len() as f32 * 40.0 + 64.0,
        Menu::Quality => (sh.heights.len() as f32 + 1.0) * 40.0 + 64.0,
        Menu::Captions => {
            let translate = v.captions.iter().any(|c| c.translatable) as usize;
            (v.captions.len() + 1 + translate) as f32 * 40.0 + 64.0
        }
        Menu::Translate(_) => 320.0f32.min(h - 140.0).max(120.0) + 64.0,
        Menu::Sleep => (SLEEP_MINUTES.len() + 2) as f32 * 40.0 + 64.0,
        Menu::Audio => v.audio_tracks.len() as f32 * 40.0 + 64.0,
        _ => {
            (4 + sh.normalization_supported as usize
                + (!sh.is_live) as usize
                + (sh.cc_count > 0) as usize
                + (!v.audio_tracks.is_empty()) as usize) as f32
                * 40.0
                + 16.0
        }
    }
}

fn caption_label(s: &App) -> String {
    let v = &s.video;
    match &v.shown.cc_sel {
        None => "Off".into(),
        Some((i, lang)) => {
            let base = v
                .captions
                .get(*i)
                .map(|c| c.label.clone())
                .unwrap_or_default();
            match lang
                .as_ref()
                .and_then(|code| v.caption_languages.iter().find(|l| &l.code == code))
            {
                Some(l) => format!("{base} >> {}", l.label),
                None => base,
            }
        }
    }
}

fn sleep_label(sl: Sleep) -> String {
    match sl {
        Sleep::Off => "Off".into(),
        Sleep::At(due, _) => {
            let left = (due - crate::scroller::now()).max(0.0);
            format!("{} min left", (left / 60.0).ceil() as u32)
        }
        Sleep::EndOfVideo => "End of video".into(),
    }
}

fn up_next(s: &App, w: f32, h: f32) -> El {
    let tok = t();
    let next = s.next_video();
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
            .child(thumb(s, &v, tw, tw * 9.0 / 16.0, 8.0, s.scale.get()))
            .child(
                text(v.title.clone())
                    .font(S16M)
                    .color(tok.text)
                    .max_lines(2)
                    .max_w((w - 48.0).max(100.0)),
            )
            .child(
                text(verified_name(&v.channel, v.channel_verified))
                    .font(S14)
                    .color(tok.text2),
            );
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
        .child(target(
            avatar(s, channel.avatar.as_deref(), 40.0, scale)
                .label(channel.name.clone())
                .cursor(Cursor::Pointer)
                .on_press_tx(open.clone()),
            "channel",
        ))
        .child(
            col()
                .min_w(0.0)
                .shrink()
                .cursor(Cursor::Pointer)
                .on_press_tx(open)
                .child(
                    text(verified_name(
                        &channel.name,
                        w.info
                            .as_ref()
                            .map_or(w.video.channel_verified, |i| i.channel_verified),
                    ))
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

/// The chapter list under the open description: thumbnail, title and start; a press seeks
/// there and brings the player into view.
pub fn chapters(s: &App, page: &Page, scale: f32) -> El {
    let tok = t();
    let Some(info) = page.watch().and_then(|w| w.info.as_ref()) else {
        return el();
    };
    let cur = s.video.chapter_idx;
    let mut c = col().key("chapters").w_full().h_full().child(
        row()
            .h(CHAPTERS_HEAD)
            .center_y()
            .child(text("Chapters").font(S16M).color(tok.text)),
    );
    for (i, ch) in info.chapters.iter().enumerate() {
        let start = ch.start;
        let mut thumb = el().w(96.0).h(54.0).radius(8.0).fill(tok.skeleton);
        if let Some(u) = ch.thumbnail.as_deref().filter(|u| !u.is_empty()) {
            thumb = thumb
                .pixels_fit(
                    &s.images.get(u, crate::view::px(96.0, scale)),
                    SurfaceFit::Fill,
                )
                .clip_rounded(8.0);
        }
        c = c.child(focus(
            row()
                .key(format!("chapter-{i}"))
                .w_full()
                .h(CHAPTER_ROW)
                .px(4.0)
                .gap(12.0)
                .center_y()
                .radius(8.0)
                .role("button")
                .label(ch.title.clone())
                .cursor(Cursor::Pointer)
                .fill(if cur == Some(i) {
                    tok.fill
                } else {
                    Color::TRANSPARENT
                })
                .hover(tok.fill)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    s.video.seek(start);
                    s.top_mut()
                        .scroll
                        .anim
                        .scroll_to(0.0, crate::scroller::now());
                    tx.wake_after(std::time::Duration::ZERO);
                })
                .child(thumb)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(text(ch.title.clone()).font(S14M).color(tok.text).truncate())
                        .child(
                            text(nt_core::format::clock(ch.start))
                                .font(S12)
                                .color(tok.link),
                        ),
                ),
        ));
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
    let mut header = row()
        .key("comments-head")
        .w_full()
        .h_full()
        .center_y()
        .gap(12.0)
        .child(text(label).font(S20B).color(tok.text));
    if !w.comments_sort.is_empty() {
        header = header.child(text("Sort by").font(S14M).color(tok.text2));
        for (i, sort) in w.comments_sort.iter().enumerate() {
            let label = if sort.label.starts_with("Top") {
                "Top"
            } else if sort.label.starts_with("Newest") {
                "Newest"
            } else {
                &sort.label
            };
            header = header.child(target(
                focus(pill(label, None, w.comment_order == Some(i)))
                    .key(format!("comment-sort-{i}"))
                    .label(label)
                    .on_press(move |s: &mut App| s.sort_comments(i)),
                "comment_sort",
            ));
        }
    }
    header
}

pub fn related_chips(page: &Page) -> El {
    let mut chips = row().w_full().h_full().gap(6.0).center_y().clip_soft(0.0);
    let Some(watch) = page.watch() else {
        return chips;
    };
    let Some(info) = &watch.info else {
        return chips;
    };
    for (i, chip) in info.related_chips.iter().enumerate() {
        let selected = watch.related_chip.map_or(chip.selected, |index| index == i);
        chips = chips.child(target(
            focus(pill(&chip.label, None, selected))
                .key(format!("related-chip-{i}"))
                .label(chip.label.clone())
                .on_press(move |s: &mut App| s.related_chip(i)),
            "chip",
        ));
    }
    chips
}

pub fn replies_more(page: &Page, i: usize) -> El {
    let Some(w) = page.watch() else { return el() };
    let Some(c) = w.comments.get(i) else {
        return el();
    };
    let Some(thread) = w.replies.get(&c.id) else {
        return el();
    };
    let id = c.id.clone();
    match &thread.state {
        Load::Loading => el()
            .h_full()
            .center_y()
            .child(Spinner::new().size(20.0).build(kit())),
        Load::Idle | Load::Error(_) => {
            let label = if matches!(thread.state, Load::Error(_)) {
                "Couldn't load replies · Retry"
            } else {
                "Show more replies"
            };
            target(
                focus(pill(label, None, false))
                    .label(label)
                    .on_press(move |s: &mut App| s.load_replies(id.clone())),
                "comment_replies",
            )
        }
    }
}

pub fn comment(
    s: &App,
    page: &Page,
    c: &nt_core::Comment,
    i: usize,
    b: &Block,
    scale: f32,
    reply: bool,
) -> El {
    let tok = t();
    let open = reply
        || page
            .watch()
            .is_some_and(|w| w.comments_open.contains(&c.id));
    let full = measure(&c.text, S14, comment_text_w(b.w));
    let clamped = !reply && full > 80.5;
    let name = text(verified_name(&c.author, c.author_verified))
        .font(S13M)
        .color(tok.text)
        .truncate()
        .shrink();
    // The channel owner's own comment: the name in a grey pill, as YouTube marks it.
    let name = if c.by_creator {
        row()
            .px(6.0)
            .radius(12.0)
            .fill(tok.fill)
            .shrink()
            .min_w(0.0)
            .child(name)
    } else {
        name
    };
    let mut head = row().gap(4.0).center_y().child(name);
    if let Some(p) = &c.published {
        head = head.child(text(p.clone()).font(S12).color(tok.text2));
    }
    let mut body = text(c.text.clone()).font(S14).color(tok.text).wrap();
    if !open {
        body = body.max_lines(4);
    }
    let mut col_ = col().grow(1.0).min_w(0.0).gap(2.0);
    if c.pinned {
        let owner = page.watch().map_or_else(String::new, |w| w.channel().name);
        col_ = col_.child(
            text(format!("Pinned by {owner}"))
                .font(S12)
                .color(tok.text2),
        );
    }
    col_ = col_.child(head).child(body);
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
    if c.hearted {
        let owner = page.watch().map(Watch::channel);
        foot = foot.child(
            row()
                .gap(2.0)
                .center_y()
                .label("Hearted by the creator")
                .child(avatar(
                    s,
                    owner.as_ref().and_then(|ch| ch.avatar.as_deref()),
                    20.0,
                    scale,
                ))
                .child(text("♥").font(S14M).color(tok.red)),
        );
    }
    col_ = col_.child(foot);
    if !reply && c.replies.is_some() {
        let open = page
            .watch()
            .and_then(|w| w.replies.get(&c.id))
            .is_some_and(|r| r.open);
        let id = c.id.clone();
        let label = if open {
            "Hide replies".into()
        } else {
            c.reply_count.as_deref().map_or_else(
                || "Show replies".into(),
                |n| format!("{n} {}", if n == "1" { "reply" } else { "replies" }),
            )
        };
        col_ = col_.child(target(
            focus(pill(&label, Some(&CHEVRON_RIGHT), false))
                .label(label)
                .key(format!("replies-{}", c.id))
                .on_press(move |s: &mut App| s.toggle_replies(id.clone())),
            "comment_replies",
        ));
    }
    row()
        .key(format!("cm-{i}-{}", c.id))
        .w_full()
        .gap(16.0)
        .child(avatar(s, c.author_avatar.as_deref(), 40.0, scale))
        .child(col_)
}

pub fn playlist_heading(page: &Page) -> El {
    let Some(panel) = page.watch().and_then(|w| w.playlist.as_ref()) else {
        return el();
    };
    row()
        .h_full()
        .w_full()
        .px(8.0)
        .center_y()
        .gap(8.0)
        .fill(t().fill)
        .radius(8.0)
        .child(
            text(panel.title.clone())
                .font(S16M)
                .color(t().text)
                .grow(1.0)
                .truncate(),
        )
        .child(
            text(format!(
                "{} / {}",
                panel.index.map_or(0, |i| i + 1),
                panel.items.len()
            ))
            .font(S12M)
            .color(t().text2),
        )
}

pub fn playlist_row(s: &App, page: &Page, index: usize, scale: f32) -> El {
    let Some(panel) = page.watch().and_then(|w| w.playlist.as_ref()) else {
        return el();
    };
    let Some(video) = panel.items.get(index) else {
        return el();
    };
    let tok = t();
    target(
        focus(
            row()
                .key(format!("playlist-row-{index}"))
                .w_full()
                .h_full()
                .px(6.0)
                .gap(8.0)
                .center_y()
                .radius(8.0)
                .role("link")
                .label(video.title.clone())
                .cursor(Cursor::Pointer)
                .fill(if panel.index == Some(index) {
                    tok.fill
                } else {
                    Color::TRANSPARENT
                })
                .hover(tok.fill_hover)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.play_playlist_item(index, tx))
                .child(
                    text((index + 1).to_string())
                        .w(25.0)
                        .font(S12)
                        .color(tok.text2),
                )
                .child(thumb(s, video, 90.0, 51.0, 4.0, scale))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(3.0)
                        .child(
                            text(video.title.clone())
                                .font(S14M)
                                .color(tok.text)
                                .max_lines(2),
                        )
                        .child(
                            text(video.channel.clone())
                                .font(S12)
                                .color(tok.text2)
                                .truncate(),
                        ),
                ),
        ),
        "row",
    )
}

pub fn compact(s: &App, v: &Video, i: usize, b: &Block, scale: f32) -> El {
    let tok = t();
    let open = v.clone();
    // Navigation freezes the outgoing page at its presented offset. Do not reject a
    // selected recommendation just because the wheel animation still has velocity.
    let go = move |s: &mut App, tx: &mut Tx| s.open_video(open.clone(), tx);
    // The whole row is one link: it takes the press, the focus and the right-click; nothing
    // inside it is a separate press target, so a click anywhere on it opens the video.
    target(
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
                            text(verified_name(&v.channel, v.channel_verified))
                                .font(S12)
                                .color(tok.text2)
                                .truncate(),
                        )
                        .child(
                            text(crate::view::video_meta(v))
                                .font(S12)
                                .color(tok.text2)
                                .truncate(),
                        )
                        .child(badges(v)),
                )
                .child(more_button(v, [b.x + b.w - 32.0, b.y]).mr(-8.0)),
        ),
        "related",
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

// ----- player right-click menu and stats -------------------------------------------------

pub const PLAYER_MENU_W: f32 = 264.0;
const PLAYER_MENU_ITEMS: usize = 4;

/// The player's right-click menu (YouTube's): Loop, Copy video URL, Copy video URL at current
/// time, Stats for nerds. At the pointer, kept inside the window; a press elsewhere closes it
/// and does nothing else.
pub fn player_menu(s: &App, at: [f32; 2]) -> El {
    let v = &s.video;
    let id = v.id.clone().unwrap_or_default();
    let item = |key: &str, ic: &'static Icon, label: &'static str, check: Option<bool>| {
        let mut r = row()
            .key(key.to_string())
            .w_full()
            .h(40.0)
            .px(12.0)
            .gap(12.0)
            .center_y()
            .role("menuitem")
            .label(label)
            .cursor(Cursor::Pointer)
            .hover(Color::WHITE.alpha(0.1))
            .focusable(true)
            .focus_highlight(Color::WHITE.alpha(0.1))
            .child(icon(ic, 24.0).color(Color::WHITE))
            .child(text(label).font(S14).color(Color::WHITE).grow(1.0));
        if let Some(on) = check {
            r = r.child(if on {
                icon(&CHECK, 20.0).color(Color::WHITE)
            } else {
                el().w(20.0)
            });
        }
        r
    };
    let (id1, id2) = (id.clone(), id);
    let menu = col()
        .key("player-menu")
        .role("menu")
        .w(PLAYER_MENU_W)
        .py(8.0)
        .radius(12.0)
        .fill(rune::color::hex("#1c1c1ce6").unwrap_or(Color::BLACK))
        .shadow(4.0, 32.0, Color::BLACK.alpha(0.5))
        .focus_group()
        .on_press(|_: &mut App| {})
        .child(
            item("pm-loop", &LOOP, "Loop", Some(v.shown.looping)).on_press(|s: &mut App| {
                s.video.toggle_loop();
                s.player_menu = None;
            }),
        )
        .child(item("pm-url", &SHARE, "Copy video URL", None).on_press_tx(
            move |s: &mut App, tx: &mut Tx| {
                s.player_menu = None;
                tx.copy(format!("https://www.youtube.com/watch?v={id1}"));
                s.show_toast("Link copied to clipboard", tx);
            },
        ))
        .child(
            item(
                "pm-url-time",
                &LATER,
                "Copy video URL at current time",
                None,
            )
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.player_menu = None;
                let t = s.video.position.max(0.0).floor() as u64;
                tx.copy(format!("https://www.youtube.com/watch?v={id2}&t={t}s"));
                s.show_toast("Link copied to clipboard", tx);
            }),
        )
        .child(
            item("pm-stats", &INFO, "Stats for nerds", None).on_press(|s: &mut App| {
                s.player_menu = None;
                s.video.stats_open = true;
                s.video.sync();
            }),
        );
    let h = 16.0 + 40.0 * PLAYER_MENU_ITEMS as f32;
    let [x, y] = popup_at(at, PLAYER_MENU_W, h, s.win_w, s.win_h);
    el().key("player-menu-layer")
        .transition(rune::Transition::none())
        .instant_changes()
        .modal_tx(false, |s: &mut App, tx| {
            s.player_menu = None;
            tx.instant();
        })
        .absolute(0.0, 0.0)
        .w(s.win_w)
        .h(s.win_h)
        .on_press(|s: &mut App| s.player_menu = None)
        .on_secondary_press(|s: &mut App, _, _: &mut Tx| s.player_menu = None)
        .child(menu.absolute(x, y))
}

/// Where a popup `w` × `h` opened at the pointer `at` goes: there, flipped left or up near the
/// right or bottom edge, and always 8 points inside the window.
pub fn popup_at(at: [f32; 2], w: f32, h: f32, win_w: f32, win_h: f32) -> [f32; 2] {
    let x = if at[0] + w > win_w - 8.0 {
        at[0] - w
    } else {
        at[0]
    };
    let y = if at[1] + h > win_h - 8.0 {
        at[1] - h
    } else {
        at[1]
    };
    [
        x.clamp(8.0, (win_w - w - 8.0).max(8.0)),
        y.clamp(8.0, (win_h - h - 8.0).max(8.0)),
    ]
}

/// "Stats for nerds": the player's live numbers in a box over the player's top-left, with ×.
fn stats_overlay(s: &App, w: f32) -> El {
    let bw = 380.0f32.min(w - 20.0).max(200.0);
    let mut rows = col().gap(2.0).max_w(bw - 36.0);
    for line in &s.video.stats_rows {
        rows = rows.child(
            // A live text paints in the box its placeholder reserves: give it the row's size.
            text(" ")
                .live_text(line)
                .font(S12)
                .color(Color::WHITE)
                .size(bw - 36.0, 18.0),
        );
    }
    el().key("stats")
        .absolute(10.0, 10.0)
        .w(bw)
        .padding(10.0)
        .radius(4.0)
        .fill(Color::BLACK.alpha(0.7))
        .on_press(|_: &mut App| {})
        .child(rows)
        .child(
            el().key("stats-close")
                .absolute(bw - 26.0, 4.0)
                .size(22.0, 22.0)
                .center()
                .role("button")
                .label("Close stats")
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut App| s.video.stats_open = false)
                .child(icon(&CLOSE, 18.0).color(Color::WHITE)),
        )
}
