//! Process startup configuration for the native GPU renderer.

#[cfg(target_os = "linux")]
mod linux;

/// Select the GLVND EGL vendor for the Wayland compositor's main DRM device on Linux.
/// Explicit vendor settings and unsupported or ambiguous environments retain the loader default.
/// This is a no-op on other platforms.
///
/// # Safety
///
/// Call on the initial thread before any other thread can start or any code can independently
/// read or write the process environment. Call before initializing the window toolkit or EGL.
/// The Linux checks are a guard against late invocation, not synchronization.
pub unsafe fn select_egl_vendor() {
    #[cfg(target_os = "linux")]
    {
        if !linux::single_threaded() || !linux::egl_unloaded() {
            eprintln!("nt-core: EGL vendor selection skipped outside single-threaded startup");
            return;
        }
        if let Some(path) = linux::manifest() {
            if !linux::single_threaded() {
                eprintln!("nt-core: EGL vendor selection skipped because another thread started");
                return;
            }
            // SAFETY: the caller guarantees exclusive startup access to the environment.
            // The Wayland probe is synchronous and the process still has one thread.
            unsafe { std::env::set_var("__EGL_VENDOR_LIBRARY_FILENAMES", path) };
        }
    }
}
