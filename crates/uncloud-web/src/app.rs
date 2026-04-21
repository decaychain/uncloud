use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use crate::hooks::{use_auth, use_search, use_storages};
use crate::router::Route;
use crate::state::{AuthState, FontScale, HighlightTarget, PlayerState, RescanState, ThemeState, VaultOpenTarget, ViewMode};

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const FAVICON: Asset = asset!("/assets/favicon.ico");
const FAVICON_PNG: Asset = asset!("/assets/favicon-32.png");
const APPLE_TOUCH_ICON: Asset = asset!("/assets/apple-touch-icon.png");

#[component]
pub fn App() -> Element {
    // Start in loading state; check for an existing session before rendering.
    let mut auth_state = use_context_provider(|| Signal::new(AuthState { loading: true, user: None }));
    let mut search_enabled = use_context_provider(|| Signal::new(false));

    use_effect(move || {
        spawn(async move {
            if let Ok(user) = use_auth::me().await {
                auth_state.write().user = Some(user);
                // Existing session — fetch search availability now.
                let enabled = use_search::fetch_search_enabled().await;
                search_enabled.set(enabled);
            }
            auth_state.write().loading = false;
        });
    });

    let is_dark = use_signal(|| {
        web_sys::window()
            .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
            .map(|mql| mql.matches())
            .unwrap_or(false)
    });

    let initial_font_scale: FontScale = LocalStorage::get::<String>("uncloud_font_scale")
        .ok()
        .and_then(|s| FontScale::from_str(&s))
        .unwrap_or_default();

    let theme_state = use_context_provider(move || {
        Signal::new(ThemeState {
            dark: is_dark(),
            font_scale: initial_font_scale,
        })
    });

    // Apply the font scale to the document root so all Tailwind rem-based
    // sizes scale uniformly. Re-runs whenever the preference changes.
    use_effect(move || {
        let px = theme_state().font_scale.px();
        if let Some(html) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = html.set_attribute("style", &format!("font-size: {}px", px));
        }
    });

    let initial_view_mode = LocalStorage::get::<String>("uncloud_view_mode")
        .ok()
        .and_then(|s| if s == "list" { Some(ViewMode::List) } else { None })
        .unwrap_or_default();

    let initial_expand_depth: u32 = LocalStorage::get::<u32>("uncloud_music_expand_depth").unwrap_or(1);

    use_context_provider(|| Signal::new(initial_view_mode));
    use_context_provider(|| Signal::new(initial_expand_depth));
    use_context_provider(|| Signal::new(PlayerState::default()));
    use_context_provider(|| Signal::new(HighlightTarget::default()));
    use_context_provider(|| Signal::new(VaultOpenTarget::default()));
    let mut rescan_state = use_context_provider(|| Signal::new(RescanState::default()));

    // Hydrate the live rescan panel when the logged-in user is an admin.
    // A rescan may already be running (restarted session, different browser,
    // or a reload mid-scan); SSE will take over from whatever snapshot comes
    // back here.
    use_effect(move || {
        let auth = auth_state.read();
        if !auth.loading && auth.is_admin() {
            drop(auth);
            spawn(async move {
                if let Ok(Some(job)) = use_storages::get_active_rescan_job().await {
                    rescan_state.set(RescanState {
                        job: Some(job),
                        error: None,
                        starting: false,
                    });
                }
            });
        }
    });

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Link { rel: "icon", href: FAVICON, sizes: "16x16 32x32 48x48" }
        document::Link { rel: "icon", href: FAVICON_PNG, r#type: "image/png", sizes: "32x32" }
        document::Link { rel: "apple-touch-icon", href: APPLE_TOUCH_ICON }
        Router::<Route> {}
    }
}
