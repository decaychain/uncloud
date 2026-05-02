#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Disable WebKit GPU compositing to avoid Wayland protocol errors on some
    // driver/mesa configurations.
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

    // Skip GTK's AT-SPI accessibility bridge. The bridge initializes
    // dbus on a thread that races libdbus's global lock setup; on
    // Fedora 43 this races consistently and segfaults inside
    // pthread_mutex_lock with a NULL mutex (atspi_get_a11y_bus →
    // dbus_connection_allocate_data_slot → _dbus_lock). Disabling the
    // bridge sidesteps the race entirely.
    //
    // Cost: screen readers (Orca etc.) won't see the app's UI. The
    // tradeoff is acceptable for now — Tauri apps generally don't
    // surface accessibility metadata to AT-SPI anyway because the UI
    // lives inside a WebKit view.
    #[cfg(target_os = "linux")]
    std::env::set_var("NO_AT_BRIDGE", "1");

    uncloud_desktop_lib::run();
}
