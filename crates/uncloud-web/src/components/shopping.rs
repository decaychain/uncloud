use std::collections::HashMap;

use dioxus::prelude::*;
use crate::components::icons::IconSettings;
use uncloud_common::{
    AddShoppingListItemRequest, CategoryResponse, CreateShopRequest, PatchShoppingListItemRequest,
    ShopResponse, ShoppingItemResponse, ShoppingListItemResponse, ShoppingListResponse,
    ShoppingListSummary, UpdateShopRequest, UpdateShoppingItemRequest,
};

use crate::hooks::use_shopping;
use crate::router::Route;

// ── ShoppingPage ─────────────────────────────────────────────────────────

#[component]
pub fn ShoppingPage() -> Element {
    let mut lists: Signal<Vec<ShoppingListSummary>> = use_signal(Vec::new);
    let mut categories: Signal<Vec<CategoryResponse>> = use_signal(Vec::new);
    let mut shops: Signal<Vec<ShopResponse>> = use_signal(Vec::new);
    let mut all_list_data: Signal<HashMap<String, ShoppingListResponse>> =
        use_signal(HashMap::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    // Three independent filter signals
    let mut selected_list: Signal<Option<String>> = use_signal(|| None);
    let mut selected_shop: Signal<Option<String>> = use_signal(|| None);
    let mut selected_category: Signal<Option<String>> = use_signal(|| None);

    // Inline inlet: which item's inlet is open
    let open_inlet: Signal<Option<String>> = use_signal(|| None);

    // Create inline state
    let mut creating = use_signal(|| false);
    let mut new_name: Signal<String> = use_signal(String::new);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    // Delete confirm: Some((id, name))
    let mut delete_target: Signal<Option<(String, String)>> = use_signal(|| None);

    // Rename state: Some((id, current_name))
    let mut rename_target: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut rename_name: Signal<String> = use_signal(String::new);
    let mut rename_error: Signal<Option<String>> = use_signal(|| None);

    // Share state: Some(list_id)
    let mut share_target: Signal<Option<String>> = use_signal(|| None);
    let mut share_username: Signal<String> = use_signal(String::new);
    let mut share_error: Signal<Option<String>> = use_signal(|| None);
    let mut all_usernames: Signal<Vec<String>> = use_signal(Vec::new);

    // Settings panel
    let mut show_settings = use_signal(|| false);

    // Load all data. Only show the loading spinner on the very first fetch
    // (when we have no data yet). Subsequent refreshes update in-place without
    // hiding the existing UI, eliminating flicker.
    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            let is_initial = lists().is_empty() && all_list_data().is_empty();
            if is_initial {
                loading.set(true);
            }
            // Load lists
            match use_shopping::list_lists().await {
                Ok(l) => {
                    // For each list, load items
                    let mut data_map = HashMap::new();
                    for list in &l {
                        if let Ok(list_data) = use_shopping::get_list(&list.id).await {
                            data_map.insert(list.id.clone(), list_data);
                        }
                    }
                    all_list_data.set(data_map);
                    lists.set(l);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            // Load categories
            if let Ok(cats) = use_shopping::list_categories().await {
                categories.set(cats);
            }
            // Load shops
            if let Ok(s) = use_shopping::list_shops().await {
                shops.set(s);
            }
            loading.set(false);
        });
    });

    let list_data = lists();
    let cats = categories();
    let shops_data = shops();
    let all_data = all_list_data();

    let sel_list = selected_list();
    let sel_shop = selected_shop();
    let sel_category = selected_category();

    // Gather all items across all lists
    let all_items: Vec<(String, String, ShoppingListItemResponse)> = all_data
        .iter()
        .flat_map(|(list_id, list_resp)| {
            list_resp.items.iter().map(move |item| {
                (
                    list_id.clone(),
                    list_resp.name.clone(),
                    item.clone(),
                )
            })
        })
        .collect();

    // Filter items: all three filters AND together
    let filtered_items: Vec<(String, String, ShoppingListItemResponse)> = all_items
        .iter()
        .filter(|(list_id, _, item)| {
            // List filter
            if let Some(ref lid) = sel_list {
                if list_id != lid {
                    return false;
                }
            }
            // Category filter
            if let Some(ref cat) = sel_category {
                if !item.categories.contains(cat) {
                    return false;
                }
            }
            // Shop filter: tier 1 (explicit shop_ids) or tier 2 (category-inferred)
            if let Some(ref shop_id) = sel_shop {
                let direct = item.shop_ids.contains(shop_id);
                let inferred = {
                    let shop_cats: Vec<String> = shops_data
                        .iter()
                        .find(|s| &s.id == shop_id)
                        .map(|s| s.categories.clone())
                        .unwrap_or_default();
                    item.categories.iter().any(|c| shop_cats.contains(c))
                };
                if !direct && !inferred {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();

    // Group by list
    let mut grouped_by_list: HashMap<String, (String, Vec<ShoppingListItemResponse>)> =
        HashMap::new();
    for (list_id, list_name, item) in &filtered_items {
        grouped_by_list
            .entry(list_id.clone())
            .or_insert_with(|| (list_name.clone(), Vec::new()))
            .1
            .push(item.clone());
    }

    rsx! {
        div { class: "flex flex-col lg:flex-row gap-4",
            // Left sidebar: list switcher
            div { class: "lg:w-64 shrink-0",
                div { class: "card bg-base-100 shadow",
                    div { class: "card-body py-3 px-3",
                        div { class: "flex items-center justify-between mb-2",
                            h3 { class: "font-bold text-sm", "Lists" }
                            button {
                                class: "btn btn-ghost btn-xs btn-circle",
                                title: "Settings",
                                onclick: move |_| show_settings.set(true),
                                IconSettings { class: "w-4 h-4".to_string() }
                            }
                        }

                        // New list input
                        div { class: "flex items-center gap-1 mb-2",
                            input {
                                class: "input input-bordered input-xs flex-1",
                                r#type: "text",
                                placeholder: "New list...",
                                value: "{new_name}",
                                oninput: move |e| {
                                    new_name.set(e.value());
                                    create_error.set(None);
                                },
                                onkeydown: move |e| {
                                    if e.key() == Key::Enter {
                                        let name = new_name().trim().to_string();
                                        if !name.is_empty() {
                                            spawn(async move {
                                                creating.set(true);
                                                match use_shopping::create_list(&name).await {
                                                    Ok(_) => {
                                                        new_name.set(String::new());
                                                        create_error.set(None);
                                                        let next = *refresh.peek() + 1;
                                                        refresh.set(next);
                                                    }
                                                    Err(e) => {
                                                        if e == "CONFLICT" {
                                                            create_error.set(Some(format!("\"{}\" already exists", name)));
                                                        } else {
                                                            create_error.set(Some(e));
                                                        }
                                                    }
                                                }
                                                creating.set(false);
                                            });
                                        }
                                    }
                                },
                            }
                            button {
                                class: "btn btn-primary btn-xs",
                                disabled: creating() || new_name().trim().is_empty(),
                                onclick: move |_| {
                                    let name = new_name().trim().to_string();
                                    if !name.is_empty() {
                                        spawn(async move {
                                            creating.set(true);
                                            match use_shopping::create_list(&name).await {
                                                Ok(_) => {
                                                    new_name.set(String::new());
                                                    create_error.set(None);
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                }
                                                Err(e) => {
                                                    if e == "CONFLICT" {
                                                        create_error.set(Some(format!("\"{}\" already exists", name)));
                                                    } else {
                                                        create_error.set(Some(e));
                                                    }
                                                }
                                            }
                                            creating.set(false);
                                        });
                                    }
                                },
                                if creating() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "+"
                            }
                        }
                        if let Some(err) = create_error() {
                            div { class: "text-error text-xs mb-1", "{err}" }
                        }

                        // List entries
                        div { class: "space-y-1",
                            for list in list_data.iter() {
                                {
                                    let list_id_filter = list.id.clone();
                                    let list_id_rename = list.id.clone();
                                    let list_name_rename = list.name.clone();
                                    let list_id_delete = list.id.clone();
                                    let list_name_delete = list.name.clone();
                                    let list_id_share = list.id.clone();
                                    let is_active = sel_list.as_ref() == Some(&list.id);
                                    let shared_info = if list.shared_with.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" (shared with {})", list.shared_with.join(", "))
                                    };
                                    rsx! {
                                        div { class: if is_active { "flex items-center gap-1 rounded px-2 py-1 bg-primary/10 group" } else { "flex items-center gap-1 rounded px-2 py-1 hover:bg-base-200 group" },
                                            button {
                                                class: "flex-1 text-left text-sm truncate",
                                                onclick: move |_| {
                                                    let current = selected_list();
                                                    if current.as_ref() == Some(&list_id_filter) {
                                                        selected_list.set(None);
                                                    } else {
                                                        selected_list.set(Some(list_id_filter.clone()));
                                                    }
                                                },
                                                span { class: "font-medium", "{list.name}" }
                                                span { class: "text-base-content/50 text-xs ml-1", "{list.item_count}" }
                                                if !shared_info.is_empty() {
                                                    span { class: "text-base-content/40 text-xs ml-1", "{shared_info}" }
                                                }
                                            }
                                            div { class: "dropdown dropdown-end",
                                                div {
                                                    tabindex: "0",
                                                    role: "button",
                                                    class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100",
                                                    "..."
                                                }
                                                ul {
                                                    tabindex: "0",
                                                    class: "dropdown-content z-10 menu menu-sm shadow bg-base-200 rounded-box w-36",
                                                    li {
                                                        a {
                                                            onclick: move |_| {
                                                                rename_name.set(list_name_rename.clone());
                                                                rename_error.set(None);
                                                                rename_target.set(Some((list_id_rename.clone(), list_name_rename.clone())));
                                                            },
                                                            "Rename"
                                                        }
                                                    }
                                                    li {
                                                        a {
                                                            onclick: move |_| {
                                                                share_username.set(String::new());
                                                                share_error.set(None);
                                                                share_target.set(Some(list_id_share.clone()));
                                                                spawn(async move {
                                                                    if let Ok(names) = use_shopping::list_usernames().await {
                                                                        all_usernames.set(names);
                                                                    }
                                                                });
                                                            },
                                                            "Share"
                                                        }
                                                    }
                                                    li {
                                                        a {
                                                            class: "text-error",
                                                            onclick: move |_| {
                                                                delete_target.set(Some((list_id_delete.clone(), list_name_delete.clone())));
                                                            },
                                                            "Delete"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Main content area
            div { class: "flex-1 min-w-0",
                if let Some(err) = error() {
                    div { class: "alert alert-error mb-4", "{err}" }
                }

                // Filter bar: three independent dropdowns
                div { class: "flex gap-2 flex-wrap mb-4",
                    select {
                        class: "select select-bordered select-sm flex-1",
                        oninput: move |e| {
                            let v = e.value();
                            selected_list.set(if v.is_empty() { None } else { Some(v) });
                        },
                        option { value: "", "All lists" }
                        for list in list_data.iter() {
                            {
                                let lid = list.id.clone();
                                let lname = list.name.clone();
                                let is_sel = sel_list.as_ref() == Some(&lid);
                                rsx! {
                                    option {
                                        value: "{lid}",
                                        selected: is_sel,
                                        "{lname}"
                                    }
                                }
                            }
                        }
                    }
                    select {
                        class: "select select-bordered select-sm flex-1",
                        oninput: move |e| {
                            let v = e.value();
                            selected_shop.set(if v.is_empty() { None } else { Some(v) });
                        },
                        option { value: "", "All shops" }
                        for shop in shops_data.iter() {
                            {
                                let sid = shop.id.clone();
                                let sname = shop.name.clone();
                                let is_sel = sel_shop.as_ref() == Some(&sid);
                                rsx! {
                                    option {
                                        value: "{sid}",
                                        selected: is_sel,
                                        "{sname}"
                                    }
                                }
                            }
                        }
                    }
                    select {
                        class: "select select-bordered select-sm flex-1",
                        oninput: move |e| {
                            let v = e.value();
                            selected_category.set(if v.is_empty() { None } else { Some(v) });
                        },
                        option { value: "", "All categories" }
                        for cat in cats.iter() {
                            {
                                let cname = cat.name.clone();
                                let is_sel = sel_category.as_ref() == Some(&cname);
                                rsx! {
                                    option {
                                        value: "{cname}",
                                        selected: is_sel,
                                        "{cname}"
                                    }
                                }
                            }
                        }
                    }
                }

                if loading() {
                    div { class: "flex justify-center py-12",
                        span { class: "loading loading-spinner loading-lg" }
                    }
                } else if filtered_items.is_empty() {
                    div { class: "card bg-base-100 shadow",
                        div { class: "card-body items-center text-center py-12",
                            p { class: "text-base-content/70",
                                if sel_list.is_some() || sel_shop.is_some() || sel_category.is_some() {
                                    "No items match the current filters."
                                } else {
                                    "No items in any list. Create a list and add items!"
                                }
                            }
                        }
                    }
                } else {
                    // Grouped display
                    div { class: "space-y-4",
                        for (list_id_key, (list_name_val, items_val)) in grouped_by_list.iter() {
                            {
                                let lid = list_id_key.clone();
                                let lname = list_name_val.clone();
                                let items_vec = items_val.clone();
                                let unchecked: Vec<ShoppingListItemResponse> = items_vec.iter().filter(|i| !i.checked).cloned().collect();
                                let checked: Vec<ShoppingListItemResponse> = items_vec.iter().filter(|i| i.checked).cloned().collect();
                                let has_removable = checked.iter().any(|i| !i.recurring);

                                rsx! {
                                    div { class: "card bg-base-100 shadow",
                                        div { class: "card-body py-3 px-0",
                                            div { class: "flex items-center justify-between px-4 mb-2",
                                                h3 { class: "font-bold text-sm", "{lname}" }
                                            }

                                            // Unchecked items
                                            for item in unchecked.iter() {
                                                {
                                                    let item_clone = item.clone();
                                                    let lid_clone = lid.clone();
                                                    rsx! {
                                                        ShoppingListItemRow {
                                                            key: "{item_clone.id}",
                                                            item: item_clone.clone(),
                                                            list_id: lid_clone,
                                                            shops: shops_data.clone(),
                                                            categories: cats.clone(),
                                                            open_inlet: open_inlet,
                                                            on_changed: move |_| {
                                                                let next = *refresh.peek() + 1;
                                                                refresh.set(next);
                                                            },
                                                        }
                                                    }
                                                }
                                            }

                                            if !checked.is_empty() {
                                                div { class: "divider text-xs text-base-content/40 my-1 px-4", "Checked" }
                                            }

                                            for item in checked.iter() {
                                                {
                                                    let item_clone = item.clone();
                                                    let lid_clone = lid.clone();
                                                    rsx! {
                                                        ShoppingListItemRow {
                                                            key: "{item_clone.id}",
                                                            item: item_clone.clone(),
                                                            list_id: lid_clone,
                                                            shops: shops_data.clone(),
                                                            categories: cats.clone(),
                                                            open_inlet: open_inlet,
                                                            on_changed: move |_| {
                                                                let next = *refresh.peek() + 1;
                                                                refresh.set(next);
                                                            },
                                                        }
                                                    }
                                                }
                                            }

                                            if has_removable {
                                                {
                                                    let lid_remove = lid.clone();
                                                    rsx! {
                                                        div { class: "px-4 pt-2",
                                                            button {
                                                                class: "btn btn-ghost btn-sm text-error",
                                                                onclick: move |_| {
                                                                    let lid = lid_remove.clone();
                                                                    spawn(async move {
                                                                        let _ = use_shopping::remove_purchased(&lid).await;
                                                                        let next = *refresh.peek() + 1;
                                                                        refresh.set(next);
                                                                    });
                                                                },
                                                                "Remove purchased"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Add item input for this list (only if viewing a specific list)
                                    if sel_list.as_ref() == Some(&lid) {
                                        {
                                            let lid_add = lid.clone();
                                            rsx! {
                                                AddItemInput {
                                                    list_id: lid_add,
                                                    selected_category: sel_category.clone(),
                                                    selected_shop: sel_shop.clone(),
                                                    open_inlet: open_inlet,
                                                    on_added: move |_| {
                                                        let next = *refresh.peek() + 1;
                                                        refresh.set(next);
                                                    },
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Show add item for specific list when filter is ByList and items were empty
                if sel_list.is_some() && filtered_items.is_empty() {
                    {
                        if let Some(ref lid) = sel_list {
                            let lid_add = lid.clone();
                            rsx! {
                                AddItemInput {
                                    list_id: lid_add,
                                    selected_category: sel_category.clone(),
                                    selected_shop: sel_shop.clone(),
                                    open_inlet: open_inlet,
                                    on_added: move |_| {
                                        let next = *refresh.peek() + 1;
                                        refresh.set(next);
                                    },
                                }
                            }
                        } else {
                            rsx! {}
                        }
                    }
                }
            }
        }

        // Rename modal
        if let Some((ref target_id, _)) = rename_target() {
            {
                let tid = target_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-4", "Rename List" }
                            if let Some(err) = rename_error() {
                                div { class: "alert alert-error mb-3 text-sm", "{err}" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "text",
                                placeholder: "List name",
                                value: "{rename_name}",
                                oninput: move |e| rename_name.set(e.value()),
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| rename_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: rename_name().trim().is_empty(),
                                    onclick: move |_| {
                                        let name = rename_name().trim().to_string();
                                        let id = tid.clone();
                                        spawn(async move {
                                            match use_shopping::rename_list(&id, &name).await {
                                                Ok(_) => {
                                                    rename_target.set(None);
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                }
                                                Err(e) => rename_error.set(Some(e)),
                                            }
                                        });
                                    },
                                    "Rename"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| rename_target.set(None) }
                    }
                }
            }
        }

        // Delete confirm modal
        if let Some((ref del_id, ref del_name)) = delete_target() {
            {
                let did = del_id.clone();
                let dname = del_name.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-2", "Delete List" }
                            p { class: "text-base-content/70",
                                "Delete \"{dname}\"? This will remove all items from the list."
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| delete_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        let id = did.clone();
                                        spawn(async move {
                                            let _ = use_shopping::delete_list(&id).await;
                                            delete_target.set(None);
                                            selected_list.set(None);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                        });
                                    },
                                    "Delete"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| delete_target.set(None) }
                    }
                }
            }
        }

        // Share modal
        if let Some(ref share_list_id) = share_target() {
            {
                let slid = share_list_id.clone();
                let slid2 = share_list_id.clone();
                let current_shares: Vec<String> = lists()
                    .iter()
                    .find(|l| l.id == slid)
                    .map(|l| l.shared_with.clone())
                    .unwrap_or_default();

                // Filter out users already shared with
                let available_users: Vec<String> = all_usernames()
                    .into_iter()
                    .filter(|u| !current_shares.contains(u))
                    .collect();

                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-4", "Share List" }
                            if let Some(err) = share_error() {
                                div { class: "alert alert-error mb-3 text-sm", "{err}" }
                            }

                            if !current_shares.is_empty() {
                                div { class: "mb-4",
                                    p { class: "text-sm font-medium mb-2", "Shared with:" }
                                    div { class: "space-y-1",
                                        for username in current_shares.iter() {
                                            {
                                                let uname = username.clone();
                                                let uname_rm = username.clone();
                                                let slid_rm = slid2.clone();
                                                rsx! {
                                                    div { class: "flex items-center justify-between bg-base-200 rounded px-3 py-1",
                                                        span { class: "text-sm", "{uname}" }
                                                        button {
                                                            class: "btn btn-ghost btn-xs text-error",
                                                            title: "Remove",
                                                            onclick: move |_| {
                                                                let lid = slid_rm.clone();
                                                                let uid = uname_rm.clone();
                                                                spawn(async move {
                                                                    let _ = use_shopping::unshare_list(&lid, &uid).await;
                                                                    let next = *refresh.peek() + 1;
                                                                    refresh.set(next);
                                                                });
                                                            },
                                                            "\u{00D7}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if available_users.is_empty() {
                                p { class: "text-sm text-base-content/50", "No more users to share with." }
                            } else {
                                div { class: "flex items-center gap-2",
                                    select {
                                        class: "select select-bordered select-sm flex-1",
                                        value: "{share_username}",
                                        oninput: move |e| {
                                            share_username.set(e.value());
                                            share_error.set(None);
                                        },
                                        option { value: "", disabled: true, "Select a user..." }
                                        for uname in available_users.iter() {
                                            {
                                                let u = uname.clone();
                                                rsx! {
                                                    option { value: "{u}", "{u}" }
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn btn-primary btn-sm",
                                        disabled: share_username().trim().is_empty(),
                                        onclick: move |_| {
                                            let username = share_username().trim().to_string();
                                            let lid = slid.clone();
                                            spawn(async move {
                                                match use_shopping::share_list(&lid, &username).await {
                                                    Ok(_) => {
                                                        share_username.set(String::new());
                                                        share_error.set(None);
                                                        let next = *refresh.peek() + 1;
                                                        refresh.set(next);
                                                    }
                                                    Err(e) => share_error.set(Some(e)),
                                                }
                                            });
                                        },
                                        "Share"
                                    }
                                }
                            }

                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| share_target.set(None),
                                    "Close"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| share_target.set(None) }
                    }
                }
            }
        }

        // Settings modal
        if show_settings() {
            SettingsPanel {
                on_close: move |_| {
                    show_settings.set(false);
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                },
            }
        }
    }
}

// ── ShoppingListView (route-based, kept for backward compat) ────────────

#[component]
pub fn ShoppingListView(list_id: String) -> Element {
    let nav = use_navigator();
    use_effect(move || {
        nav.push(Route::Shopping {});
    });
    rsx! {
        div { class: "flex justify-center py-12",
            span { class: "loading loading-spinner loading-lg" }
        }
    }
}

// ── AddItemInput ────────────────────────────────────────────────────────

#[component]
fn AddItemInput(
    list_id: String,
    selected_category: Option<String>,
    selected_shop: Option<String>,
    open_inlet: Signal<Option<String>>,
    on_added: EventHandler<()>,
) -> Element {
    let mut add_input: Signal<String> = use_signal(String::new);
    let mut adding = use_signal(|| false);
    let mut add_error: Signal<Option<String>> = use_signal(|| None);

    let mut catalogue: Signal<Vec<ShoppingItemResponse>> = use_signal(Vec::new);
    let mut show_suggestions = use_signal(|| false);

    let mut list_id_sig = use_signal(|| list_id.clone());
    if *list_id_sig.peek() != list_id {
        list_id_sig.set(list_id.clone());
    }

    let mut sel_cat_sig: Signal<Option<String>> = use_signal(|| selected_category.clone());
    if *sel_cat_sig.peek() != selected_category {
        sel_cat_sig.set(selected_category.clone());
    }

    let mut sel_shop_sig: Signal<Option<String>> = use_signal(|| selected_shop.clone());
    if *sel_shop_sig.peek() != selected_shop {
        sel_shop_sig.set(selected_shop.clone());
    }

    use_effect(move || {
        spawn(async move {
            if let Ok(items) = use_shopping::list_items().await {
                catalogue.set(items);
            }
        });
    });

    rsx! {
        div { class: "card bg-base-100 shadow mt-2",
            div { class: "card-body py-3",
                div { class: "relative",
                    div { class: "flex items-center gap-2",
                        input {
                            class: "input input-bordered input-sm flex-1",
                            r#type: "text",
                            placeholder: "Add item...",
                            value: "{add_input}",
                            oninput: move |e| {
                                add_input.set(e.value());
                                add_error.set(None);
                                show_suggestions.set(!e.value().trim().is_empty());
                            },
                            onfocusin: move |_| {
                                if !add_input().trim().is_empty() {
                                    show_suggestions.set(true);
                                }
                            },
                            onfocusout: move |_| {
                                spawn(async move {
                                    gloo_timers::future::TimeoutFuture::new(200).await;
                                    show_suggestions.set(false);
                                });
                            },
                            onkeydown: move |e| {
                                if e.key() == Key::Enter {
                                    let name = add_input().trim().to_string();
                                    if !name.is_empty() {
                                        let lid = list_id_sig();
                                        let cat = sel_cat_sig();
                                        let shop = sel_shop_sig();
                                        spawn(async move {
                                            adding.set(true);
                                            add_error.set(None);
                                            let req = AddShoppingListItemRequest {
                                                item_id: None,
                                                name: Some(name),
                                                categories: cat.into_iter().collect(),
                                                shop_ids: shop.into_iter().collect(),
                                                quantity: None,
                                                recurring: false,
                                            };
                                            match use_shopping::add_list_item(&lid, req).await {
                                                Ok(new_item) => {
                                                    add_input.set(String::new());
                                                    show_suggestions.set(false);
                                                    open_inlet.set(Some(new_item.id));
                                                    on_added.call(());
                                                    if let Ok(items) = use_shopping::list_items().await {
                                                        catalogue.set(items);
                                                    }
                                                }
                                                Err(e) => add_error.set(Some(e)),
                                            }
                                            adding.set(false);
                                        });
                                    }
                                }
                            },
                        }
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: adding() || add_input().trim().is_empty(),
                            onclick: move |_| {
                                let name = add_input().trim().to_string();
                                if !name.is_empty() {
                                    let lid = list_id_sig();
                                    let cat = sel_cat_sig();
                                    let shop = sel_shop_sig();
                                    spawn(async move {
                                        adding.set(true);
                                        add_error.set(None);
                                        let req = AddShoppingListItemRequest {
                                            item_id: None,
                                            name: Some(name),
                                            categories: cat.into_iter().collect(),
                                            shop_ids: shop.into_iter().collect(),
                                            quantity: None,
                                            recurring: false,
                                        };
                                        match use_shopping::add_list_item(&lid, req).await {
                                            Ok(new_item) => {
                                                add_input.set(String::new());
                                                show_suggestions.set(false);
                                                open_inlet.set(Some(new_item.id));
                                                on_added.call(());
                                                if let Ok(items) = use_shopping::list_items().await {
                                                    catalogue.set(items);
                                                }
                                            }
                                            Err(e) => add_error.set(Some(e)),
                                        }
                                        adding.set(false);
                                    });
                                }
                            },
                            if adding() {
                                span { class: "loading loading-spinner loading-xs" }
                            }
                            "Add"
                        }
                    }

                    // Suggestions dropdown
                    if show_suggestions() {
                        {
                            let query = add_input().to_lowercase();
                            let suggestions: Vec<ShoppingItemResponse> = catalogue()
                                .into_iter()
                                .filter(|item| item.name.to_lowercase().contains(&query))
                                .take(8)
                                .collect();

                            if !suggestions.is_empty() {
                                rsx! {
                                    ul { class: "menu bg-base-200 rounded-box shadow-lg absolute z-50 w-full mt-1 max-h-48 overflow-y-auto",
                                        for suggestion in suggestions {
                                            {
                                                let s_id = suggestion.id.clone();
                                                let s_name = suggestion.name.clone();
                                                let s_cats = suggestion.categories.clone();
                                                rsx! {
                                                    li {
                                                        a {
                                                            onclick: move |_| {
                                                                let item_id = s_id.clone();
                                                                let lid = list_id_sig();
                                                                spawn(async move {
                                                                    adding.set(true);
                                                                    add_error.set(None);
                                                                    let req = AddShoppingListItemRequest {
                                                                        item_id: Some(item_id),
                                                                        name: None,
                                                                        categories: Vec::new(),
                                                                        shop_ids: Vec::new(),
                                                                        quantity: None,
                                                                        recurring: false,
                                                                    };
                                                                    match use_shopping::add_list_item(&lid, req).await {
                                                                        Ok(new_item) => {
                                                                            add_input.set(String::new());
                                                                            show_suggestions.set(false);
                                                                            open_inlet.set(Some(new_item.id));
                                                                            on_added.call(());
                                                                        }
                                                                        Err(e) => add_error.set(Some(e)),
                                                                    }
                                                                    adding.set(false);
                                                                });
                                                            },
                                                            span { class: "flex-1", "{s_name}" }
                                                            for cat in s_cats.iter() {
                                                                {
                                                                    let c = cat.clone();
                                                                    rsx! {
                                                                        span { class: "badge badge-sm badge-ghost", "{c}" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                rsx! {}
                            }
                        }
                    }
                }
                if let Some(err) = add_error() {
                    div { class: "text-error text-sm mt-1", "{err}" }
                }
            }
        }
    }
}

// ── ShoppingListItemRow ──────────────────────────────────────────────────

#[component]
fn ShoppingListItemRow(
    item: ShoppingListItemResponse,
    list_id: String,
    shops: Vec<ShopResponse>,
    categories: Vec<CategoryResponse>,
    open_inlet: Signal<Option<String>>,
    on_changed: EventHandler<()>,
) -> Element {
    let item_id_toggle = item.id.clone();
    let item_id_remove = item.id.clone();
    let item_id_recurring = item.id.clone();
    let item_id_bring_top = item.id.clone();
    let item_id_inlet = item.id.clone();
    let list_id_toggle = list_id.clone();
    let list_id_remove = list_id.clone();
    let list_id_recurring = list_id.clone();
    let list_id_bring_top = list_id.clone();
    let is_checked = item.checked;
    let is_recurring = item.recurring;

    // Resolve shop names
    let shop_names: Vec<String> = item
        .shop_ids
        .iter()
        .filter_map(|sid| shops.iter().find(|s| &s.id == sid).map(|s| s.name.clone()))
        .collect();

    let name_class = if is_checked && !is_recurring {
        "flex-1 line-through text-base-content/40 cursor-pointer hover:underline"
    } else if is_checked && is_recurring {
        "flex-1 opacity-50 cursor-pointer hover:underline"
    } else {
        "flex-1 cursor-pointer hover:underline"
    };

    let is_inlet_open = open_inlet() == Some(item.id.clone());

    rsx! {
        div {
            div { class: "flex items-center gap-3 px-4 py-2 hover:bg-base-200 transition-colors group",
                input {
                    r#type: "checkbox",
                    class: "checkbox checkbox-sm",
                    checked: is_checked,
                    onchange: move |_| {
                        let lid = list_id_toggle.clone();
                        let iid = item_id_toggle.clone();
                        let new_checked = !is_checked;
                        spawn(async move {
                            let req = PatchShoppingListItemRequest {
                                checked: Some(new_checked),
                                quantity: None,
                                recurring: None,
                            };
                            if use_shopping::patch_list_item(&lid, &iid, req).await.is_ok() {
                                on_changed.call(());
                            }
                        });
                    },
                }

                // Recurring toggle
                button {
                    class: if is_recurring { "btn btn-ghost btn-xs btn-circle text-info" } else { "btn btn-ghost btn-xs btn-circle text-base-content/30" },
                    title: if is_recurring { "Recurring (click to toggle)" } else { "Not recurring (click to toggle)" },
                    onclick: move |_| {
                        let lid = list_id_recurring.clone();
                        let iid = item_id_recurring.clone();
                        let new_recurring = !is_recurring;
                        spawn(async move {
                            let req = PatchShoppingListItemRequest {
                                checked: None,
                                quantity: None,
                                recurring: Some(new_recurring),
                            };
                            if use_shopping::patch_list_item(&lid, &iid, req).await.is_ok() {
                                on_changed.call(());
                            }
                        });
                    },
                    "\u{21BA}"
                }

                // Clickable name that toggles the inlet
                span {
                    class: name_class,
                    onclick: move |_| {
                        let id = item_id_inlet.clone();
                        if open_inlet() == Some(id.clone()) {
                            open_inlet.set(None);
                        } else {
                            open_inlet.set(Some(id));
                        }
                    },
                    "{item.name}"
                }

                if let Some(ref qty) = item.quantity {
                    span { class: "badge badge-sm badge-outline", "{qty}" }
                }
                for cat in item.categories.iter() {
                    {
                        let c = cat.clone();
                        rsx! {
                            span { class: "badge badge-sm badge-ghost", "{c}" }
                        }
                    }
                }
                for sname in shop_names.iter() {
                    {
                        let s = sname.clone();
                        rsx! {
                            span { class: "badge badge-sm badge-info badge-outline", "{s}" }
                        }
                    }
                }

                // Context menu
                div { class: "dropdown dropdown-end",
                    div {
                        tabindex: "0",
                        role: "button",
                        class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100",
                        "..."
                    }
                    ul {
                        tabindex: "0",
                        class: "dropdown-content z-10 menu menu-sm shadow bg-base-200 rounded-box w-40",
                        li {
                            a {
                                onclick: move |_| {
                                    let lid = list_id_bring_top.clone();
                                    let iid = item_id_bring_top.clone();
                                    spawn(async move {
                                        let _ = use_shopping::update_item_position(&lid, &iid, -1.0).await;
                                        on_changed.call(());
                                    });
                                },
                                "Bring to top"
                            }
                        }
                        li {
                            a {
                                class: "text-error",
                                onclick: move |_| {
                                    let lid = list_id_remove.clone();
                                    let iid = item_id_remove.clone();
                                    spawn(async move {
                                        if use_shopping::remove_list_item(&lid, &iid).await.is_ok() {
                                            on_changed.call(());
                                        }
                                    });
                                },
                                "Remove from list"
                            }
                        }
                    }
                }
            }

            // Inline inlet beneath the item row
            if is_inlet_open {
                ItemInlet {
                    item: item.clone(),
                    categories: categories.clone(),
                    shops: shops.clone(),
                    on_dismiss: move |_| open_inlet.set(None),
                    on_updated: move |_| {
                        on_changed.call(());
                    },
                }
            }
        }
    }
}

// ── ItemInlet ────────────────────────────────────────────────────────────

#[component]
fn ItemInlet(
    item: ShoppingListItemResponse,
    categories: Vec<CategoryResponse>,
    shops: Vec<ShopResponse>,
    on_dismiss: EventHandler<()>,
    on_updated: EventHandler<()>,
) -> Element {
    let mut item_name: Signal<String> = use_signal(|| item.name.clone());
    let mut selected_categories: Signal<Vec<String>> =
        use_signal(|| item.categories.clone());
    let mut selected_shop_ids: Signal<Vec<String>> =
        use_signal(|| item.shop_ids.clone());
    let mut quantity: Signal<String> =
        use_signal(|| item.quantity.clone().unwrap_or_default());
    let mut saving = use_signal(|| false);

    let catalogue_item_id = item.item_id.clone();

    rsx! {
        div {
            class: "pl-8 pr-4 pb-3 pt-1 bg-base-200/50 border-l-2 border-primary/30",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_dismiss.call(());
                }
            },

            // Name row
            div { class: "mb-2 flex items-center gap-2",
                span { class: "text-xs font-medium text-base-content/60", "Name:" }
                input {
                    class: "input input-bordered input-xs flex-1",
                    r#type: "text",
                    value: "{item_name}",
                    oninput: move |e| item_name.set(e.value()),
                }
            }

            // Categories row
            div { class: "mb-2",
                span { class: "text-xs font-medium text-base-content/60 mr-2", "Categories:" }
                div { class: "inline-flex flex-wrap gap-1",
                    for cat in categories.iter() {
                        {
                            let cn = cat.name.clone();
                            let cn2 = cat.name.clone();
                            let is_selected = selected_categories().contains(&cn);
                            rsx! {
                                button {
                                    class: if is_selected { "badge badge-primary cursor-pointer" } else { "badge badge-outline cursor-pointer" },
                                    onclick: move |_| {
                                        let mut current = selected_categories();
                                        if current.contains(&cn2) {
                                            current.retain(|c| c != &cn2);
                                        } else {
                                            current.push(cn2.clone());
                                        }
                                        selected_categories.set(current);
                                    },
                                    "{cn}"
                                }
                            }
                        }
                    }
                }
            }

            // Shops row
            div { class: "mb-2",
                span { class: "text-xs font-medium text-base-content/60 mr-2", "Shops:" }
                div { class: "inline-flex flex-wrap gap-1",
                    for shop in shops.iter() {
                        {
                            let sid = shop.id.clone();
                            let sid2 = shop.id.clone();
                            let sname = shop.name.clone();
                            let is_selected = selected_shop_ids().contains(&sid);
                            rsx! {
                                button {
                                    class: if is_selected { "badge badge-info cursor-pointer" } else { "badge badge-outline cursor-pointer" },
                                    onclick: move |_| {
                                        let mut current = selected_shop_ids();
                                        if current.contains(&sid2) {
                                            current.retain(|s| s != &sid2);
                                        } else {
                                            current.push(sid2.clone());
                                        }
                                        selected_shop_ids.set(current);
                                    },
                                    "{sname}"
                                }
                            }
                        }
                    }
                }
            }

            // Quantity row
            div { class: "mb-2 flex items-center gap-2",
                span { class: "text-xs font-medium text-base-content/60", "Qty:" }
                input {
                    class: "input input-bordered input-xs w-24",
                    r#type: "text",
                    placeholder: "e.g. 2x, 500g",
                    value: "{quantity}",
                    oninput: move |e| quantity.set(e.value()),
                }
            }

            // Save / Cancel buttons
            div { class: "flex gap-2",
                button {
                    class: "btn btn-primary btn-xs",
                    disabled: saving(),
                    onclick: move |_| {
                        let item_id = catalogue_item_id.clone();
                        let new_name = item_name().trim().to_string();
                        let cats = selected_categories();
                        let shop_ids = selected_shop_ids();
                        spawn(async move {
                            saving.set(true);
                            // Update the catalogue item's name, categories and shops
                            let _ = use_shopping::update_item(
                                &item_id,
                                UpdateShoppingItemRequest {
                                    name: if new_name.is_empty() { None } else { Some(new_name) },
                                    categories: Some(cats),
                                    shop_ids: Some(shop_ids),
                                    notes: None,
                                },
                            )
                            .await;
                            saving.set(false);
                            on_updated.call(());
                            on_dismiss.call(());
                        });
                    },
                    if saving() {
                        span { class: "loading loading-spinner loading-xs" }
                    }
                    "Save"
                }
                button {
                    class: "btn btn-ghost btn-xs",
                    onclick: move |_| on_dismiss.call(()),
                    "Cancel"
                }
            }
        }
    }
}

// ── SettingsPanel ────────────────────────────────────────────────────────

#[component]
fn SettingsPanel(on_close: EventHandler<()>) -> Element {
    let mut categories: Signal<Vec<CategoryResponse>> = use_signal(Vec::new);
    let mut shops: Signal<Vec<ShopResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut refresh = use_signal(|| 0u32);

    // New category
    let mut new_cat_name: Signal<String> = use_signal(String::new);
    let mut cat_error: Signal<Option<String>> = use_signal(|| None);

    // Rename category: Some((id, current_name))
    let mut rename_cat_target: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut rename_cat_name: Signal<String> = use_signal(String::new);
    let mut rename_cat_error: Signal<Option<String>> = use_signal(|| None);

    // New shop
    let mut new_shop_name: Signal<String> = use_signal(String::new);
    let mut shop_error: Signal<Option<String>> = use_signal(|| None);

    // Edit shop: Some((id, name, categories))
    let mut edit_shop: Signal<Option<(String, String, Vec<String>)>> = use_signal(|| None);
    let mut edit_shop_name: Signal<String> = use_signal(String::new);
    let mut edit_shop_cats: Signal<Vec<String>> = use_signal(Vec::new);
    let mut edit_error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            if let Ok(cats) = use_shopping::list_categories().await {
                categories.set(cats);
            }
            if let Ok(s) = use_shopping::list_shops().await {
                shops.set(s);
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-lg",
                h3 { class: "font-bold text-lg mb-4", "Shopping Settings" }

                if loading() {
                    div { class: "flex justify-center py-8",
                        span { class: "loading loading-spinner loading-lg" }
                    }
                } else {
                    // Categories section with ordering
                    div { class: "mb-6",
                        h4 { class: "font-medium mb-2", "Categories" }
                        div { class: "space-y-1 mb-2",
                            {
                                let cats_list = categories();
                                let cats_len = cats_list.len();
                                rsx! {
                                    for (i, cat) in cats_list.iter().enumerate() {
                                        {
                                            let cid = cat.id.clone();
                                            let cname = cat.name.clone();
                                            let cid_up = cat.id.clone();
                                            let cid_down = cat.id.clone();
                                            let can_up = i > 0;
                                            let can_down = i < cats_len - 1;

                                            // Calculate positions for reordering
                                            let cats_for_up = cats_list.clone();
                                            let cats_for_down = cats_list.clone();
                                            let idx = i;

                                            let cid_rename = cat.id.clone();
                                            let cname_rename = cat.name.clone();

                                            rsx! {
                                                div { class: "flex items-center gap-2 bg-base-200 rounded px-3 py-1",
                                                    span {
                                                        class: "flex-1 text-sm cursor-pointer hover:underline",
                                                        title: "Click to rename",
                                                        onclick: move |_| {
                                                            rename_cat_name.set(cname_rename.clone());
                                                            rename_cat_error.set(None);
                                                            rename_cat_target.set(Some((cid_rename.clone(), cname_rename.clone())));
                                                        },
                                                        "{cname}"
                                                    }
                                                    if can_up {
                                                        button {
                                                            class: "btn btn-ghost btn-xs",
                                                            onclick: move |_| {
                                                                let id = cid_up.clone();
                                                                let prev_pos = cats_for_up[idx - 1].position;
                                                                let new_pos = prev_pos - 0.5;
                                                                spawn(async move {
                                                                    let _ = use_shopping::update_category_position(&id, new_pos).await;
                                                                    let next = *refresh.peek() + 1;
                                                                    refresh.set(next);
                                                                });
                                                            },
                                                            "\u{25B2}"
                                                        }
                                                    }
                                                    if can_down {
                                                        button {
                                                            class: "btn btn-ghost btn-xs",
                                                            onclick: move |_| {
                                                                let id = cid_down.clone();
                                                                let next_pos = cats_for_down[idx + 1].position;
                                                                let new_pos = next_pos + 0.5;
                                                                spawn(async move {
                                                                    let _ = use_shopping::update_category_position(&id, new_pos).await;
                                                                    let next = *refresh.peek() + 1;
                                                                    refresh.set(next);
                                                                });
                                                            },
                                                            "\u{25BC}"
                                                        }
                                                    }
                                                    button {
                                                        class: "btn btn-ghost btn-xs text-error",
                                                        onclick: move |_| {
                                                            let id = cid.clone();
                                                            spawn(async move {
                                                                let _ = use_shopping::delete_category(&id).await;
                                                                let next = *refresh.peek() + 1;
                                                                refresh.set(next);
                                                            });
                                                        },
                                                        "\u{00D7}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center gap-2",
                            input {
                                class: "input input-bordered input-sm flex-1",
                                r#type: "text",
                                placeholder: "New category...",
                                value: "{new_cat_name}",
                                oninput: move |e| {
                                    new_cat_name.set(e.value());
                                    cat_error.set(None);
                                },
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                disabled: new_cat_name().trim().is_empty(),
                                onclick: move |_| {
                                    let name = new_cat_name().trim().to_string();
                                    spawn(async move {
                                        match use_shopping::create_category(&name).await {
                                            Ok(_) => {
                                                new_cat_name.set(String::new());
                                                cat_error.set(None);
                                                let next = *refresh.peek() + 1;
                                                refresh.set(next);
                                            }
                                            Err(e) => cat_error.set(Some(e)),
                                        }
                                    });
                                },
                                "Add"
                            }
                        }
                        if let Some(err) = cat_error() {
                            div { class: "text-error text-xs mt-1", "{err}" }
                        }
                    }

                    // Shops section
                    div { class: "mb-4",
                        h4 { class: "font-medium mb-2", "Shops" }
                        div { class: "space-y-1 mb-2",
                            for shop in shops().iter() {
                                {
                                    let _sid = shop.id.clone();
                                    let sname = shop.name.clone();
                                    let scats = shop.categories.clone();
                                    let sid_edit = shop.id.clone();
                                    let sname_edit = shop.name.clone();
                                    let scats_edit = shop.categories.clone();
                                    let sid_del = shop.id.clone();
                                    let cats_display = if scats.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" ({})", scats.join(", "))
                                    };
                                    rsx! {
                                        div { class: "flex items-center justify-between bg-base-200 rounded px-3 py-1",
                                            div { class: "flex-1 min-w-0",
                                                span { class: "text-sm font-medium", "{sname}" }
                                                if !cats_display.is_empty() {
                                                    span { class: "text-xs text-base-content/50", "{cats_display}" }
                                                }
                                            }
                                            div { class: "flex gap-1",
                                                button {
                                                    class: "btn btn-ghost btn-xs",
                                                    onclick: move |_| {
                                                        edit_shop_name.set(sname_edit.clone());
                                                        edit_shop_cats.set(scats_edit.clone());
                                                        edit_error.set(None);
                                                        edit_shop.set(Some((sid_edit.clone(), sname_edit.clone(), scats_edit.clone())));
                                                    },
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-xs text-error",
                                                    onclick: move |_| {
                                                        let id = sid_del.clone();
                                                        spawn(async move {
                                                            let _ = use_shopping::delete_shop(&id).await;
                                                            let next = *refresh.peek() + 1;
                                                            refresh.set(next);
                                                        });
                                                    },
                                                    "\u{00D7}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center gap-2",
                            input {
                                class: "input input-bordered input-sm flex-1",
                                r#type: "text",
                                placeholder: "New shop...",
                                value: "{new_shop_name}",
                                oninput: move |e| {
                                    new_shop_name.set(e.value());
                                    shop_error.set(None);
                                },
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                disabled: new_shop_name().trim().is_empty(),
                                onclick: move |_| {
                                    let name = new_shop_name().trim().to_string();
                                    spawn(async move {
                                        match use_shopping::create_shop(CreateShopRequest {
                                            name,
                                            categories: Vec::new(),
                                        })
                                        .await
                                        {
                                            Ok(_) => {
                                                new_shop_name.set(String::new());
                                                shop_error.set(None);
                                                let next = *refresh.peek() + 1;
                                                refresh.set(next);
                                            }
                                            Err(e) => shop_error.set(Some(e)),
                                        }
                                    });
                                },
                                "Add"
                            }
                        }
                        if let Some(err) = shop_error() {
                            div { class: "text-error text-xs mt-1", "{err}" }
                        }
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }

        // Rename category modal
        if let Some((ref cat_id, _)) = rename_cat_target() {
            {
                let cid = cat_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-4", "Rename Category" }
                            if let Some(err) = rename_cat_error() {
                                div { class: "alert alert-error mb-3 text-sm", "{err}" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "text",
                                placeholder: "Category name",
                                value: "{rename_cat_name}",
                                oninput: move |e| rename_cat_name.set(e.value()),
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| rename_cat_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: rename_cat_name().trim().is_empty(),
                                    onclick: move |_| {
                                        let name = rename_cat_name().trim().to_string();
                                        let id = cid.clone();
                                        spawn(async move {
                                            match use_shopping::rename_category(&id, &name).await {
                                                Ok(_) => {
                                                    rename_cat_target.set(None);
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                }
                                                Err(e) => rename_cat_error.set(Some(e)),
                                            }
                                        });
                                    },
                                    "Rename"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| rename_cat_target.set(None) }
                    }
                }
            }
        }

        // Edit shop modal
        if let Some((ref eid, _, _)) = edit_shop() {
            {
                let edit_id = eid.clone();
                let all_cats = categories();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-4", "Edit Shop" }
                            if let Some(err) = edit_error() {
                                div { class: "alert alert-error mb-3 text-sm", "{err}" }
                            }
                            div { class: "form-control mb-3",
                                label { class: "label", span { class: "label-text", "Name" } }
                                input {
                                    class: "input input-bordered",
                                    r#type: "text",
                                    value: "{edit_shop_name}",
                                    oninput: move |e| edit_shop_name.set(e.value()),
                                }
                            }
                            div { class: "form-control mb-3",
                                label { class: "label", span { class: "label-text", "Categories this shop carries" } }
                                div { class: "flex flex-wrap gap-2",
                                    for cat in all_cats.iter() {
                                        {
                                            let cn = cat.name.clone();
                                            let cn2 = cat.name.clone();
                                            let is_selected = edit_shop_cats().contains(&cn);
                                            rsx! {
                                                label { class: "flex items-center gap-1 cursor-pointer",
                                                    input {
                                                        r#type: "checkbox",
                                                        class: "checkbox checkbox-sm",
                                                        checked: is_selected,
                                                        onchange: move |_| {
                                                            let mut current = edit_shop_cats();
                                                            if current.contains(&cn2) {
                                                                current.retain(|c| c != &cn2);
                                                            } else {
                                                                current.push(cn2.clone());
                                                            }
                                                            edit_shop_cats.set(current);
                                                        },
                                                    }
                                                    span { class: "text-sm", "{cn}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| edit_shop.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    onclick: move |_| {
                                        let id = edit_id.clone();
                                        let name = edit_shop_name().trim().to_string();
                                        let cats = edit_shop_cats();
                                        spawn(async move {
                                            match use_shopping::update_shop(
                                                &id,
                                                UpdateShopRequest {
                                                    name: Some(name),
                                                    categories: Some(cats),
                                                },
                                            )
                                            .await
                                            {
                                                Ok(_) => {
                                                    edit_shop.set(None);
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                }
                                                Err(e) => edit_error.set(Some(e)),
                                            }
                                        });
                                    },
                                    "Save"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| edit_shop.set(None) }
                    }
                }
            }
        }
    }
}
