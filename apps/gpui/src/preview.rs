//! A single delayed, silent MPV player shared by hoverable thumbnails.
use gpui::prelude::*;
use gpui::{Context, ObjectFit, RenderImage, Window, div, img};
use nt_core::{PixelFormat, Player};
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
pub struct HoverPreview {
    player: Option<Player>,
    pending: Option<gpui::Task<()>>,
    requested: Option<String>,
    after: u64,
    picture: Option<Arc<RenderImage>>,
    retired: Option<Arc<RenderImage>>,
}

impl HoverPreview {
    pub fn request(&mut self, id: String, width: u32, cx: &mut Context<Self>) {
        self.stop(cx);
        self.requested = Some(id.clone());
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(850))
                .await;
            let _ = this.update(cx, |p, cx| p.start(&id, width, cx));
        }));
    }

    fn start(&mut self, id: &str, width: u32, cx: &mut Context<Self>) {
        if self.requested.as_deref() != Some(id) {
            return;
        }
        if self.player.is_none() {
            let (tx, rx) = async_channel::bounded::<()>(1);
            let Ok(player) = Player::new_preview(PixelFormat::Bgra, move || {
                let _ = tx.try_send(());
            }) else {
                return;
            };
            self.player = Some(player);
            cx.spawn(async move |this, cx| {
                while rx.recv().await.is_ok() {
                    if this
                        .update(cx, |p, cx| {
                            if p.requested.is_some()
                                && p.player
                                    .as_ref()
                                    .is_some_and(|player| player.frame_generation() != p.after)
                            {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        if let Some(p) = &self.player {
            p.set_target_size(width, width * 9 / 16);
            self.after = p.frame_generation();
            let _ = p.load(id);
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        if self.requested.take().is_some() {
            if let Some(p) = &self.player {
                p.stop();
                self.after = p.frame_generation();
            }
            self.retired = self.picture.take();
            cx.notify();
        }
    }

    pub fn free_retired(&mut self, window: &mut Window) {
        if let Some(picture) = self.retired.take() {
            let _ = window.drop_image(picture);
        }
    }
}

impl Render for HoverPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.free_retired(window);
        if self.requested.is_some()
            && let Some(frame) = self.player.as_ref().and_then(Player::frame)
            && frame.generation != self.after
        {
            self.after = frame.generation;
            if let Some(buf) =
                image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels().to_vec())
            {
                let picture = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                    buf
                )]));
                if let Some(old) = self.picture.replace(picture) {
                    let _ = window.drop_image(old);
                }
            }
        }
        div()
            .size_full()
            .when_some(self.picture.clone(), |d, picture| {
                d.child(img(picture).size_full().object_fit(ObjectFit::Cover))
            })
    }
}
