#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Disable WebKit GPU compositing to avoid Wayland protocol errors on some
    // driver/mesa configurations.
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    uncloud_desktop_lib::run();
}
