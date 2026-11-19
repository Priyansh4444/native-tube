//! Keep the screen from blanking while a video plays (viewing review P1-7). On Linux this asks the
//! desktop through the xdg-desktop-portal Inhibit interface; on other platforms it does nothing.

#[cfg(target_os = "linux")]
mod imp {
    use ashpd::desktop::Request;
    use ashpd::desktop::inhibit::{InhibitFlags, InhibitOptions, InhibitProxy};
    use gpui::Context;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Default)]
    pub struct Inhibit {
        want: bool,
        /// Bumped on every change, so an answer that arrives after the wish changed is undone.
        generation: Rc<Cell<u64>>,
        held: Rc<RefCell<Option<Request<()>>>>,
        unavailable: Rc<Cell<bool>>,
    }

    impl Inhibit {
        pub fn set<T: 'static>(&mut self, on: bool, cx: &mut Context<T>) {
            if on == self.want || self.unavailable.get() {
                return;
            }
            self.want = on;
            let generation = self.generation.get().wrapping_add(1);
            self.generation.set(generation);
            let (current, held, unavailable) = (
                self.generation.clone(),
                self.held.clone(),
                self.unavailable.clone(),
            );
            cx.spawn(async move |_, _| {
                let old = held.borrow_mut().take();
                if let Some(old) = old {
                    let _ = old.close().await;
                }
                if !on {
                    return;
                }
                let result = async {
                    let proxy = InhibitProxy::new().await?;
                    proxy
                        .inhibit(
                            None,
                            InhibitFlags::Idle.into(),
                            InhibitOptions::default().set_reason("Playing a video"),
                        )
                        .await
                }
                .await;
                match result {
                    Ok(req) if current.get() == generation => *held.borrow_mut() = Some(req),
                    Ok(req) => {
                        let _ = req.close().await;
                    }
                    Err(e) => {
                        // No portal (or no Inhibit in it): say so once and stop asking.
                        eprintln!("nt-gpui: can't keep the screen awake: {e}");
                        unavailable.set(true);
                    }
                }
            })
            .detach();
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use gpui::Context;

    #[derive(Default)]
    pub struct Inhibit;

    impl Inhibit {
        pub fn set<T: 'static>(&mut self, _on: bool, _cx: &mut Context<T>) {}
    }
}

pub use imp::Inhibit;
