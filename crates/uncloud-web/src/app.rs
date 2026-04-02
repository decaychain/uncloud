use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use crate::hooks::{use_auth, use_search};
use crate::router::Route;
use crate::state::{AuthState, HighlightTarget, PlayerState, ThemeState, ViewMode};

const TAILWIND: Asset = asset!("/assets/tailwind.css");

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

    use_context_provider(move || Signal::new(ThemeState { dark: is_dark() }));

    let initial_view_mode = LocalStorage::get::<String>("uncloud_view_mode")
        .ok()
        .and_then(|s| if s == "list" { Some(ViewMode::List) } else { None })
        .unwrap_or_default();

    let initial_expand_depth: u32 = LocalStorage::get::<u32>("uncloud_music_expand_depth").unwrap_or(1);

    use_context_provider(|| Signal::new(initial_view_mode));
    use_context_provider(|| Signal::new(initial_expand_depth));
    use_context_provider(|| Signal::new(PlayerState::default()));
    use_context_provider(|| Signal::new(HighlightTarget::default()));

    rsx! {
        document::Stylesheet { href: TAILWIND }
        Router::<Route> {}
    }
}
