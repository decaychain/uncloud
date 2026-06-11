use crate::components::{
    auth::{Login, Register},
    dashboard::DashboardPage,
    duplicates::DuplicatesPage,
    file_browser::FileBrowser,
    finance::{
        FinanceAccountsPage, FinanceCategoriesPage, FinanceImportsPage, FinanceRulesPage,
        FinanceSchemasPage, FinanceSettlementsPage, FinanceTransactionsPage,
    },
    gallery::{Gallery, GalleryAlbum},
    layout::Layout,
    mail::MailPage,
    music::{
        FolderTreeView, Music, MusicAlbumView as MusicAlbumViewComp, MusicArtistView,
        MusicPlaylistView, MusicScopeCategoryView, MusicScopeFolderView,
    },
    oauth_consent::OAuthConsent,
    passwords::PasswordsPage,
    settings::SettingsPage,
    setup::Setup,
    shares_page::SharesPage,
    shopping,
    tasks::{TasksAssignedPage, TasksProjectPage, TasksSchedulePage},
    trash::Trash,
};
use dioxus::prelude::*;
use gloo_storage::Storage;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/login")]
    Login {},

    #[route("/register")]
    Register {},

    #[route("/invite/:token")]
    InviteRegister { token: String },

    // First-run onboarding (Tauri desktop only, shown when no config saved).
    #[route("/setup")]
    Setup {},

    // OAuth consent — outside the main layout so the page is a focused
    // dialog. Auth-gated inside the component (redirects to /login).
    #[route("/oauth/authorize")]
    OAuthAuthorize {},

    #[layout(Layout)]
        #[route("/")]
        Home {},

        #[route("/dashboard")]
        Dashboard {},

        #[route("/folder/:id")]
        Folder { id: String },

        #[route("/shares")]
        Shares {},

        #[route("/gallery")]
        Gallery {},

        #[route("/gallery/album/:id")]
        GalleryAlbum { id: String },

        #[route("/music")]
        Music {},

        #[route("/music/artist/:name")]
        MusicArtist { name: String },

        #[route("/music/album/:artist/:album")]
        MusicAlbum { artist: String, album: String },

        #[route("/music/folder/:id")]
        MusicFolder { id: String },

        #[route("/music/scope/folder/:id")]
        MusicScopeFolder { id: String },

        #[route("/music/scope/category/:id")]
        MusicScopeCategory { id: String },

        #[route("/music/playlist/:id")]
        MusicPlaylist { id: String },

        #[route("/trash")]
        Trash {},

        #[route("/duplicates")]
        Duplicates {},

        #[route("/passwords")]
        Passwords {},

        #[route("/tasks")]
        Tasks {},

        #[route("/tasks/assigned")]
        TasksAssigned {},

        #[route("/tasks/project/:id")]
        TasksProject { id: String },

        #[route("/finance")]
        Finance {},

        #[route("/finance/accounts")]
        FinanceAccounts {},

        #[route("/finance/categories")]
        FinanceCategories {},

        #[route("/finance/settlements")]
        FinanceSettlements {},

        #[route("/finance/settlements/:id")]
        FinanceSettlementDetail { id: String },

        #[route("/finance/schemas")]
        FinanceSchemas {},

        #[route("/finance/imports")]
        FinanceImports {},

        #[route("/finance/rules")]
        FinanceRules {},

        #[route("/shopping")]
        Shopping {},

        #[route("/shopping/list/:id")]
        ShoppingList { id: String },

        #[route("/mail")]
        Mail {},

        #[route("/mail/account/:account_id")]
        MailAccount { account_id: String },

        #[route("/settings")]
        Settings {},

        #[route("/settings/:tab")]
        SettingsTab { tab: String },
    #[end_layout]

    #[route("/share/:token")]
    PublicShare { token: String },
}

#[component]
fn Home() -> Element {
    // On mobile, the first visit to `/` in a session bounces to the Dashboard.
    // Tapping "Files" in the sidebar afterwards works normally because the
    // session flag is already set.
    let nav = use_navigator();
    use_effect(move || {
        let already = gloo_storage::SessionStorage::get::<String>("uc_landed")
            .ok()
            .is_some();
        if already {
            return;
        }
        let _ = gloo_storage::SessionStorage::set("uc_landed", "1");
        let is_mobile = web_sys::window()
            .and_then(|w| w.match_media("(max-width: 1023px)").ok().flatten())
            .map(|mql| mql.matches())
            .unwrap_or(false);
        if is_mobile {
            nav.replace(Route::Dashboard {});
        }
    });

    rsx! {
        FileBrowser { parent_id: None }
    }
}

#[component]
fn Dashboard() -> Element {
    rsx! {
        DashboardPage {}
    }
}

#[component]
fn Settings() -> Element {
    let nav = use_navigator();
    nav.replace(Route::SettingsTab {
        tab: "account".to_string(),
    });
    rsx! {}
}

#[component]
fn InviteRegister(token: String) -> Element {
    rsx! {
        Register { invite_token: Some(token) }
    }
}

#[component]
fn SettingsTab(tab: String) -> Element {
    rsx! {
        SettingsPage { tab }
    }
}

#[component]
fn Folder(id: String) -> Element {
    rsx! {
        FileBrowser { key: "{id}", parent_id: Some(id) }
    }
}

#[component]
fn Shares() -> Element {
    rsx! {
        SharesPage {}
    }
}

#[component]
fn MusicArtist(name: String) -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "p-4",
            MusicArtistView {
                name,
                on_back: move |_| { let _ = nav.push(Route::Music {}); },
                on_album_select: move |album: uncloud_common::MusicAlbumResponse| {
                    let _ = nav.push(Route::MusicAlbum {
                        artist: album.artist,
                        album: album.name,
                    });
                },
            }
        }
    }
}

#[component]
fn MusicAlbum(artist: String, album: String) -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "p-4",
            MusicAlbumViewComp {
                artist,
                album,
                on_back: move |_| { let _ = nav.push(Route::Music {}); },
            }
        }
    }
}

#[component]
fn MusicFolder(id: String) -> Element {
    let scoped_id = id.clone();
    rsx! {
        div { class: "p-3 space-y-3 sm:p-4 sm:space-y-4",
            div { class: "flex flex-wrap items-center justify-between gap-3",
                h1 { class: "text-2xl font-bold", "Music" }
                Link {
                    to: Route::MusicScopeFolder { id: scoped_id },
                    class: "btn btn-ghost btn-sm",
                    "Library view"
                }
            }
            FolderTreeView { key: "{id}", root_folder_id: Some(id) }
        }
    }
}

#[component]
fn MusicScopeFolder(id: String) -> Element {
    rsx! {
        MusicScopeFolderView { key: "{id}", id }
    }
}

#[component]
fn MusicScopeCategory(id: String) -> Element {
    rsx! {
        MusicScopeCategoryView { key: "{id}", id }
    }
}

#[component]
fn MusicPlaylist(id: String) -> Element {
    rsx! {
        MusicPlaylistView { key: "{id}", playlist_id: id }
    }
}

#[component]
fn Passwords() -> Element {
    rsx! {
        PasswordsPage {}
    }
}

#[component]
fn Duplicates() -> Element {
    rsx! {
        DuplicatesPage {}
    }
}

#[component]
fn Tasks() -> Element {
    rsx! {
        TasksSchedulePage {}
    }
}

#[component]
fn TasksAssigned() -> Element {
    rsx! {
        TasksAssignedPage {}
    }
}

#[component]
fn TasksProject(id: String) -> Element {
    rsx! {
        TasksProjectPage { key: "{id}", project_id: id }
    }
}

#[component]
fn Finance() -> Element {
    rsx! { FinanceTransactionsPage {} }
}

#[component]
fn FinanceAccounts() -> Element {
    rsx! { FinanceAccountsPage {} }
}

#[component]
fn FinanceCategories() -> Element {
    rsx! { FinanceCategoriesPage {} }
}

#[component]
fn FinanceSettlements() -> Element {
    rsx! { FinanceSettlementsPage { selected_id: None::<String> } }
}

#[component]
fn FinanceSettlementDetail(id: String) -> Element {
    rsx! { FinanceSettlementsPage { selected_id: Some(id) } }
}

#[component]
fn FinanceSchemas() -> Element {
    rsx! { FinanceSchemasPage {} }
}

#[component]
fn FinanceImports() -> Element {
    rsx! { FinanceImportsPage {} }
}

#[component]
fn FinanceRules() -> Element {
    rsx! { FinanceRulesPage {} }
}

#[component]
fn Shopping() -> Element {
    rsx! {
        shopping::ShoppingPage {}
    }
}

#[component]
fn ShoppingList(id: String) -> Element {
    rsx! {
        shopping::ShoppingListView { key: "{id}", list_id: id }
    }
}

#[component]
fn Mail() -> Element {
    rsx! {
        MailPage { route_account_id: None }
    }
}

#[component]
fn MailAccount(account_id: String) -> Element {
    rsx! {
        MailPage { key: "{account_id}", route_account_id: Some(account_id) }
    }
}

#[component]
fn OAuthAuthorize() -> Element {
    rsx! {
        OAuthConsent {}
    }
}

#[component]
fn PublicShare(token: String) -> Element {
    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                div { class: "card-body",
                    h1 { class: "card-title", "Shared File" }
                    p { class: "text-base-content/70", "Token: {token}" }
                }
            }
        }
    }
}
