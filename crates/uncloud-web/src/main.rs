mod app;
mod router;
mod state;
mod components;
mod hooks;

fn main() {
    console_error_panic_hook::set_once();
    // In Tauri mode we must get the server URL from the native config before
    // launching Dioxus, because the Router's first pushState would otherwise
    // clear any hash fragment we could have used. We spawn an async task so
    // the Tauri `invoke("get_config")` Promise can be awaited.
    wasm_bindgen_futures::spawn_local(async {
        if hooks::tauri::is_tauri() {
            let is_android = hooks::tauri::is_android();
            if is_android {
                // Android has no Tauri sync-engine config. Restore server URL
                // and auth token from localStorage (survives force-stop).
                if !hooks::api::restore_from_storage() {
                    hooks::tauri::mark_needs_setup();
                }
            } else {
                match hooks::tauri::get_config().await {
                    Some(cfg) => {
                        hooks::api::seed_api_base(cfg.server_url);
                    }
                    None => {
                        hooks::tauri::mark_needs_setup();
                    }
                }
            }
        }
        dioxus::launch(app::App);
    });
}
