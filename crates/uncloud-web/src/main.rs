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
                        // The native side performs an auto-login at app start
                        // and stashes the resulting session token. Pull it
                        // here so the WebView attaches `Authorization: Bearer`
                        // to every request — its own cookie jar is empty
                        // because the native client owns the cookies.
                        match hooks::tauri::get_auth_status().await {
                            Some(s) if s.token.is_some() => {
                                hooks::api::seed_auth_token(s.token.unwrap());
                            }
                            Some(s) if s.pending => {
                                // Auto-login still racing. Fall back to a
                                // previously-persisted token; the webview
                                // will reconcile on the next 401 / reload.
                                hooks::api::restore_from_storage();
                            }
                            _ => {
                                // Auto-login failed (skipped, login rejected,
                                // engine init failed). The webview must not
                                // appear authenticated — wipe any stale token
                                // so the auth flow bounces to login.
                                hooks::api::clear_auth_token();
                            }
                        }
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
