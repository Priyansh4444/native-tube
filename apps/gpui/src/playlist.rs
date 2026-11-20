//! Playlist and local-queue panels, mounted over the current page.
use crate::app::{NtApp, Overlay, now};
use crate::chrome::focus_ring;
use crate::icons::icon;
use crate::probe::{Layer, target};
use crate::theme::*;
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, MouseButton, ObjectFit, ScrollHandle, Window, canvas, div, img, point, px,
};
use nt_core::{PlaylistPanel, Video, client, library};

type Cx<'a> = Context<'a, NtApp>;

#[derive(Default)]
pub struct PlaylistPopup {
    id: String,
    seq: u64,
    panel: Option<PlaylistPanel>,
    loading: bool,
    error: bool,
    scroll: ScrollHandle,
}

impl NtApp {
    pub fn open_playlist(&mut self, id: &str, cx: &mut Cx) {
        if id.is_empty() {
            return;
        }
        let seq = self.playlist.seq.wrapping_add(1);
        self.playlist = PlaylistPopup {
            id: id.to_owned(),
            seq,
            loading: true,
            ..Default::default()
        };
        self.set_overlay(Overlay::Playlist, cx);
        let job = client().playlist(id);
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |a, cx| {
                if a.overlay != Overlay::Playlist || a.playlist.seq != seq {
                    return;
                }
                a.playlist.loading = false;
                match result {
                    Ok(panel) => a.playlist.panel = Some(panel),
                    Err(e) => {
                        eprintln!("nt-gpui: playlist: {e}");
                        a.playlist.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn more_playlist_popup(&mut self, cx: &mut Cx) {
        if self.overlay != Overlay::Playlist || self.playlist.loading {
            return;
        }
        let Some(cursor) = self.playlist.panel.as_ref().and_then(|p| p.next.clone()) else {
            return;
        };
        let seq = self.playlist.seq;
        self.playlist.loading = true;
        self.playlist.error = false;
        let job = client().more_playlist_videos(&cursor);
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |a, cx| {
                if a.overlay != Overlay::Playlist || a.playlist.seq != seq {
                    return;
                }
                let Some(panel) = &mut a.playlist.panel else {
                    return;
                };
                a.playlist.loading = false;
                match result {
                    Ok(page) => {
                        panel.items.extend(page.items);
                        panel.next = page.next.filter(|next| next != &cursor);
                    }
                    Err(e) => {
                        eprintln!("nt-gpui: more playlist videos: {e}");
                        a.playlist.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn playlist_or_queue(&mut self, cx: &mut Cx) -> AnyElement {
        let queue = self.overlay == Overlay::Queue;
        let queue_items = if queue { library().queue() } else { Vec::new() };
        let (title, items, playlist, has_more): (_, &[_], _, _) = if queue {
            ("Queue".to_string(), &queue_items, None, false)
        } else if let Some(panel) = &self.playlist.panel {
            (
                panel.title.clone(),
                &panel.items,
                Some(panel.id.clone()),
                panel.next.is_some(),
            )
        } else {
            (
                "Playlist".to_string(),
                &[],
                Some(self.playlist.id.clone()),
                false,
            )
        };
        let opening = !queue && self.playlist.panel.is_none();
        let loading = !queue && self.playlist.loading;
        let failed = !queue && self.playlist.error;
        let width = (self.geo.win_w - 32.0).clamp(240.0, 560.0);
        let h = (self.geo.win_h - 88.0).max(160.0);
        let scroll = self.playlist.scroll.clone();
        let visible_top = (-f32::from(scroll.offset().y)).max(0.0);
        let skip = ((visible_top / 64.0) as usize).saturating_sub(2);
        let end = (((visible_top + h) / 64.0) as usize)
            .saturating_add(4)
            .min(items.len());
        let mut rows = div()
            .relative()
            .flex_none()
            .w_full()
            .h(px(items.len() as f32 * 64.0));
        let time = now();
        for (i, v) in items.iter().enumerate().take(end).skip(skip) {
            let art = self.images.get(
                &v.thumbnail,
                96.0 * self.geo.scale,
                time,
                cx,
                |a| &mut a.images,
                NtApp::on_image,
            );
            let selected = v.clone();
            let id = v.id.clone();
            let list = playlist.clone();
            rows = rows.child(
                div()
                    .id(("playlist-popup-row", i))
                    .absolute()
                    .top(px(i as f32 * 64.0))
                    .left_0()
                    .right_0()
                    .h(px(64.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(8.0))
                    .rounded(px(8.0))
                    .hover(|s| s.bg(fill()))
                    .cursor_pointer()
                    .tab_index(0)
                    .focus_visible(focus_ring)
                    .children(target(
                        cx,
                        Layer::Overlay,
                        if queue {
                            "queue_video"
                        } else {
                            "playlist_video"
                        },
                        v.title.as_str(),
                    ))
                    .on_click(cx.listener(move |a, _, window, cx| {
                        a.play_from_panel(selected.clone(), list.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .w(px(96.0))
                            .h(px(54.0))
                            .flex_none()
                            .rounded(px(6.0))
                            .bg(fill())
                            .when_some(art, |d, art| {
                                d.child(
                                    img(art)
                                        .size_full()
                                        .rounded(px(6.0))
                                        .object_fit(ObjectFit::Cover),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .child(line(v.title.clone(), 14.0, 20.0, W500, text()))
                            .child(line(v.channel.clone(), 12.0, 18.0, W400, text2())),
                    )
                    .when(queue, |d| {
                        d.child(
                            div()
                                .id(("queue-remove", i))
                                .size(px(36.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .tab_index(0)
                                .focus_visible(focus_ring)
                                .hover(|s| s.bg(fill_hover()))
                                .children(target(cx, Layer::Overlay, "remove", "Remove from queue"))
                                .child(icon("close", 20.0, text()))
                                .on_click(cx.listener(move |a, _, _, cx| {
                                    cx.stop_propagation();
                                    library().remove_queued(&id);
                                    if let Some(id) = a.top().map(|p| p.id) {
                                        a.sync_next(id, cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        let label = if loading {
            "Loading…"
        } else if failed && opening {
            "Couldn't load playlist · Retry"
        } else if failed {
            "Couldn't load more · Retry"
        } else if has_more {
            "Show more videos"
        } else if items.is_empty() {
            if queue {
                "Your queue is empty"
            } else {
                "No videos"
            }
        } else {
            ""
        };
        let prefetch = has_more && !loading && !failed;
        let weak = cx.weak_entity();
        let list_end = items.len() as f32 * 64.0;
        let hook = canvas(
            |_, _, _| (),
            move |b, _, window, cx| {
                let Some(a) = weak.upgrade() else { return };
                if prefetch && list_end - visible_top < f32::from(b.size.height) + 640.0 {
                    let weak = a.downgrade();
                    window.defer(cx, move |_, cx| {
                        let _ = weak.update(cx, Self::more_playlist_popup);
                    });
                }
            },
        )
        .absolute()
        .inset_0();
        let list_id = playlist.unwrap_or_default();
        div()
            .id("playlist-panel")
            .absolute()
            .top(px(68.0))
            .right(px(16.0))
            .w(px(width))
            .max_h(px(h))
            .bg(surface())
            .rounded(px(12.0))
            .shadow_lg()
            .p(px(12.0))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(44.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(8.0))
                    .child(line(title, 20.0, 28.0, W700, text()))
                    .child(
                        div()
                            .id("playlist-close")
                            .size(px(36.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .tab_index(0)
                            .focus_visible(focus_ring)
                            .children(target(cx, Layer::Overlay, "close", "Close"))
                            .child(icon("close", 24.0, text()))
                            .on_click(cx.listener(|a, _, _, cx| a.set_overlay(Overlay::None, cx))),
                    ),
            )
            .child(
                div()
                    .id("playlist-scroll")
                    .relative()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(hook)
                    .child(rows)
                    .when(!label.is_empty(), |d| {
                        d.child(
                            div()
                                .id("playlist-more-popup")
                                .h(px(52.0))
                                .flex()
                                .items_center()
                                .px(px(12.0))
                                .when(!loading && (has_more || failed), |d| {
                                    d.cursor_pointer()
                                        .tab_index(0)
                                        .focus_visible(focus_ring)
                                        .children(target(
                                            cx,
                                            Layer::Overlay,
                                            "playlist_more",
                                            label,
                                        ))
                                        .on_click(cx.listener(move |a, _, _, cx| {
                                            if failed && opening {
                                                a.open_playlist(&list_id, cx);
                                            } else {
                                                a.more_playlist_popup(cx);
                                            }
                                        }))
                                })
                                .child(txt(label, 14.0, 20.0, W500, text2())),
                        )
                    }),
            )
            .into_any_element()
    }

    fn play_from_panel(
        &mut self,
        video: Video,
        playlist: Option<String>,
        window: &mut Window,
        cx: &mut Cx,
    ) {
        if playlist.is_none() && library().queue().first().is_some_and(|v| v.id == video.id) {
            library().pop_queued();
        }
        self.open_from_playlist(video, None, playlist, window, cx);
    }

    pub fn open_queue(&mut self, cx: &mut Cx) {
        self.playlist.scroll.set_offset(point(px(0.0), px(0.0)));
        self.set_overlay(Overlay::Queue, cx);
    }
}
