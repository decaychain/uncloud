#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Disable WebKit GPU compositing to avoid Wayland protocol errors on some
    // driver/mesa configurations.
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

    // Disable WebKit's DMA-BUF renderer on Linux. WebKit's
    // `drmMainDevice()` path creates a Wayland GL context to query the
    // DRM device, which on NVIDIA + Wayland routes through
    // libnvidia-eglcore.so → libdbus. NVIDIA's bundled dbus message
    // handling has an ABI mismatch with system libdbus that trips
    // "dbus message changed byte order since iterator was created" and
    // SIGABRTs the process. Skipping DMA-BUF lets WebKit pick a
    // different rendering backend that doesn't cross into NVIDIA's
    // dbus code.
    #[cfg(target_os = "linux")]
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");

    // Skip GTK's AT-SPI accessibility bridge. Tauri renders through
    // WebKit anyway, so the bridge wasn't doing useful screen-reader
    // work — and on Fedora 43 it consistently triggers the libdbus
    // init race below.
    #[cfg(target_os = "linux")]
    std::env::set_var("NO_AT_BRIDGE", "1");

    // Initialize libdbus's thread-safe global locks BEFORE any GTK,
    // WebKit, or NVIDIA-EGL code runs. libdbus has a long-standing race:
    // its global mutexes are populated lazily on first use, and if two
    // consumers (atk-bridge, NVIDIA's libnvidia-eglcore, etc.) race into
    // dbus_lock concurrently, one of them can find the mutex still NULL
    // and segfault inside pthread_mutex_lock. Calling
    // dbus_threads_init_default() up-front populates the globals
    // synchronously, closing the window.
    //
    // Done via dlopen so we don't take a build-time dependency on
    // dbus-devel — libdbus-1.so.3 is always present on any system that
    // can run a GTK app, and absent systems just skip the init silently.
    #[cfg(target_os = "linux")]
    init_dbus_threads();

    uncloud_desktop_lib::run();
}

#[cfg(target_os = "linux")]
fn init_dbus_threads() {
    use std::os::raw::{c_char, c_int};
    type DbusInit = extern "C" fn() -> c_int;

    unsafe {
        for name in [b"libdbus-1.so.3\0".as_ptr(), b"libdbus-1.so\0".as_ptr()] {
            let handle = libc::dlopen(name as *const c_char, libc::RTLD_LAZY | libc::RTLD_GLOBAL);
            if handle.is_null() {
                continue;
            }
            let sym = libc::dlsym(
                handle,
                b"dbus_threads_init_default\0".as_ptr() as *const c_char,
            );
            if !sym.is_null() {
                let f: DbusInit = std::mem::transmute(sym);
                f();
                return;
            }
        }
    }
}
