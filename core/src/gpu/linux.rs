//! Select a GLVND vendor before the application starts or creates any threads.
use std::path::PathBuf;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};

pub(super) fn manifest() -> Option<PathBuf> {
    // WAYLAND_SOCKET is an inherited FD consumed by connect_to_env. Leave it for the
    // application toolkit, which must be able to take ownership after startup.
    if [
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "__EGL_VENDOR_LIBRARY_DIRS",
        "__GLX_VENDOR_LIBRARY_NAME",
        "WAYLAND_SOCKET",
    ]
    .into_iter()
    .any(|key| std::env::var_os(key).is_some())
    {
        return None;
    }
    compositor_driver().as_deref().and_then(vendor_manifest)
}

pub(super) fn single_threaded() -> bool {
    std::fs::read_dir("/proc/self/task")
        .is_ok_and(|mut tasks| matches!(tasks.next(), Some(Ok(_))) && tasks.next().is_none())
}

pub(super) fn egl_unloaded() -> bool {
    std::fs::read_to_string("/proc/self/maps").is_ok_and(|maps| {
        !maps.lines().any(|line| {
            line.split_whitespace()
                .last()
                .and_then(|path| path.rsplit('/').next())
                .is_some_and(|name| name.starts_with("libEGL"))
        })
    })
}

#[derive(Default)]
struct Feedback {
    device: Option<libc::dev_t>,
    done: bool,
}
delegate_noop!(Feedback: ignore ZwpLinuxDmabufV1);
impl Dispatch<WlRegistry, GlobalListContents> for Feedback {
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
impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Feedback {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                state.device = device
                    .as_slice()
                    .try_into()
                    .ok()
                    .map(libc::dev_t::from_ne_bytes);
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => state.done = true,
            _ => {}
        }
    }
}

fn compositor_driver() -> Option<String> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<Feedback>(&connection).ok()?;
    let handle = queue.handle();
    let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&handle, 4..=4, ()).ok()?;
    let feedback = dmabuf.get_default_feedback(&handle, ());
    let mut result = Feedback::default();
    queue.roundtrip(&mut result).ok()?;
    feedback.destroy();
    dmabuf.destroy();
    let device = result.device.filter(|_| result.done)?;
    let major = libc::major(device);
    let minor = libc::minor(device);
    let driver = std::fs::read_link(format!("/sys/dev/char/{major}:{minor}/device/driver")).ok()?;
    Some(driver.file_name()?.to_str()?.to_owned())
}

fn vendor_manifest(driver: &str) -> Option<PathBuf> {
    let vendor = match driver {
        "nvidia" | "nvidia-drm" => "libEGL_nvidia.so",
        "i915" | "xe" | "amdgpu" | "radeon" | "nouveau" | "virtio_gpu" => "libEGL_mesa.so",
        _ => return None,
    };
    ["/etc/glvnd/egl_vendor.d", "/usr/share/glvnd/egl_vendor.d"]
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .find(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|json| serde_json::from_slice::<serde_json::Value>(&json).ok())
                .is_some_and(|json| {
                    json.pointer("/ICD/library_path")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|lib| std::path::Path::new(lib).file_name())
                        .and_then(|lib| lib.to_str())
                        .is_some_and(|lib| {
                            lib == vendor
                                || lib.strip_prefix(vendor).is_some_and(|s| s.starts_with('.'))
                        })
                })
        })
}
