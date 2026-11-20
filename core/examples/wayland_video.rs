//! Headless compositor smoke test for the native video surface.
//! Run through tools/headless.sh with NT_VIDEO_OVERRIDE set to a local clip.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(target_os = "linux")]
mod linux {
    use nt_core::{PixelFormat, Player, Status, VideoSurface};
    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
    };
    use std::fs::File;
    use std::os::fd::{AsFd, FromRawFd};
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_registry::WlRegistry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    };
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };

    #[derive(Default)]
    struct State {
        configured: bool,
        size: (i32, i32),
    }
    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(State: ignore WlCompositor);
    delegate_noop!(State: ignore WlSurface);
    delegate_noop!(State: ignore WlShm);
    delegate_noop!(State: ignore WlShmPool);
    delegate_noop!(State: ignore WlBuffer);
    impl Dispatch<XdgWmBase, ()> for State {
        fn event(
            _: &mut Self,
            base: &XdgWmBase,
            e: xdg_wm_base::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = e {
                base.pong(serial);
            }
        }
    }
    impl Dispatch<XdgSurface, ()> for State {
        fn event(
            state: &mut Self,
            surface: &XdgSurface,
            e: xdg_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = e {
                surface.ack_configure(serial);
                state.configured = true;
            }
        }
    }
    impl Dispatch<XdgToplevel, ()> for State {
        fn event(
            state: &mut Self,
            _: &XdgToplevel,
            e: xdg_toplevel::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_toplevel::Event::Configure { width, height, .. } = e
                && width > 0
                && height > 0
            {
                state.size = (width, height);
            }
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        let seconds = args
            .first()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10)
            .clamp(3, 60);
        let software = args.iter().any(|a| a == "--software");
        let detach = args.iter().any(|a| a == "--detach");
        let app_id = std::env::var("NT_APP_ID")?;
        if std::env::var("NT_MUTE").as_deref() != Ok("1")
            || std::env::var("NT_NO_MPRIS").as_deref() != Ok("1")
        {
            return Err("set NT_MUTE=1 and NT_NO_MPRIS=1".into());
        }
        if std::env::var("NT_VIDEO_OVERRIDE").is_err() {
            return Err("set NT_VIDEO_OVERRIDE to the local smoke-test clip".into());
        }
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ())?;
        let wm_base: XdgWmBase = globals.bind(&qh, 1..=1, ())?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let window = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = window.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id);
        toplevel.set_title("NativeTube video surface smoke test".into());
        surface.commit();
        let mut state = State {
            size: (960, 540),
            ..State::default()
        };
        while !state.configured {
            queue.roundtrip(&mut state)?;
        }
        let (w, h) = (state.size.0.clamp(1, 4096), state.size.1.clamp(1, 4096));
        let len = w * h * 4;
        // SAFETY: the successful syscall returns a new owned descriptor.
        let fd =
            unsafe { libc::memfd_create(c"nt-transparent-window".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: see above.
        let mem = unsafe { File::from_raw_fd(fd) };
        mem.set_len(len as u64)?;
        let pool = shm.create_pool(mem.as_fd(), len, &qh, ());
        let transparent = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &qh, ());
        pool.destroy();

        let player = Player::new(PixelFormat::Rgba, || {})?;
        player.set_target_size(w as u32, h as u32);
        let display = NonNull::new(conn.backend().display_ptr().cast()).ok_or("no wl_display")?;
        let parent = NonNull::new(surface.id().as_ptr().cast()).ok_or("no parent surface")?;
        let mut child = if software {
            None
        } else {
            let attaching = Instant::now();
            // SAFETY: the connection and parent outlive the explicitly detached child.
            let child = unsafe {
                VideoSurface::attach(
                    &player,
                    RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
                    RawWindowHandle::Wayland(WaylandWindowHandle::new(parent)),
                    1.0,
                )?
            };
            println!(
                "attach took {:.1} ms",
                attaching.elapsed().as_secs_f64() * 1000.0
            );
            child.set_rect(0, 0, w as u32, h as u32);
            Some(child)
        };
        window.set_window_geometry(0, 0, w, h);
        surface.attach(Some(&transparent), 0, 0);
        surface.damage(0, 0, w, h);
        surface.commit();
        conn.flush()?;

        let started = Instant::now();
        let cpu = nt_core::metrics::cpu_seconds();
        player.load("headless-smoke")?;
        let mut first = false;
        for _ in 0..seconds * 20 {
            if let Status::Failed(error) = player.state().status {
                return Err(error.into());
            }
            if let Some(error) = child.as_ref().and_then(VideoSurface::error) {
                return Err(error.into());
            }
            if !first
                && (child.as_ref().is_some_and(VideoSurface::is_presenting)
                    || player.frame().is_some())
            {
                println!(
                    "first frame after {:.0} ms, surface {w}x{h}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                first = true;
            }
            if detach
                && started.elapsed().as_secs() >= seconds / 2
                && let Some(surface) = child.take()
            {
                surface.detach();
            }
            queue.roundtrip(&mut state)?;
            std::thread::sleep(Duration::from_millis(50));
        }
        let stats = player.render_stats();
        println!(
            "mode={} frames={} software_frame={} position={:.2} elapsed={:.2}s cpu={:.2}s rss={} KiB",
            if software { "software" } else { "wayland" },
            stats.frames,
            player.frame().is_some(),
            player.state().position,
            started.elapsed().as_secs_f64(),
            nt_core::metrics::cpu_seconds() - cpu,
            nt_core::metrics::rss_bytes() / 1024
        );
        if !first || (detach && player.frame().is_none()) {
            return Err("missing rendered frame".into());
        }
        drop(child);
        drop(player);
        transparent.destroy();
        toplevel.destroy();
        window.destroy();
        surface.destroy();
        conn.flush()?;
        Ok(())
    }
}
