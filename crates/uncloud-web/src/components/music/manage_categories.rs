use dioxus::prelude::*;
use uncloud_common::MusicCategory;

use crate::hooks::use_music_categories;
use crate::state::MusicCategoryDirtyTick;

/// Modal allowing the user to toggle which categories a folder belongs to,
/// and to create new categories on the fly.
#[component]
pub fn ManageCategoriesModal(
    folder_id: String,
    folder_name: String,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut categories: Signal<Vec<MusicCategory>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut new_name: Signal<String> = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut dirty = use_context::<Signal<MusicCategoryDirtyTick>>();

    let folder_id_for_effect = folder_id.clone();
    use_effect(use_reactive!(|folder_id_for_effect| {
        let _ = folder_id_for_effect;
        spawn(async move {
            match use_music_categories::list_categories().await {
                Ok(c) => categories.set(c),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    }));

    let folder_id_toggle = folder_id.clone();
    let folder_id_create = folder_id.clone();
    let prefill = folder_name.clone();

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-1", "Categories" }
                p { class: "text-sm text-base-content/60 mb-4", "for \"{folder_name}\"" }

                if loading() {
                    div { class: "flex items-center justify-center py-6",
                        span { class: "loading loading-spinner loading-md" }
                    }
                } else {
                    if let Some(err) = error() {
                        div { class: "alert alert-error mb-3 text-sm", "{err}" }
                    }

                    div { class: "max-h-64 overflow-y-auto -mx-2 px-2 space-y-1",
                        if categories().is_empty() {
                            p { class: "text-sm text-base-content/50 italic py-2",
                                "No categories yet. Create one below."
                            }
                        }
                        for cat in categories() {
                            {
                                let cid = cat.id.clone();
                                let folder_id_t = folder_id_toggle.clone();
                                let is_member = cat.folder_ids.iter().any(|f| f == &folder_id_t);
                                let cat_name = cat.name.clone();
                                rsx! {
                                    label {
                                        class: "label cursor-pointer justify-start gap-3 hover:bg-base-200 rounded px-2 py-1.5",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-sm",
                                            checked: is_member,
                                            onchange: move |_| {
                                                let cid = cid.clone();
                                                let folder_id = folder_id_t.clone();
                                                spawn(async move {
                                                    let mut next: Vec<String> = categories
                                                        .peek()
                                                        .iter()
                                                        .find(|c| c.id == cid)
                                                        .map(|c| c.folder_ids.clone())
                                                        .unwrap_or_default();
                                                    if is_member {
                                                        next.retain(|f| f != &folder_id);
                                                    } else if !next.contains(&folder_id) {
                                                        next.push(folder_id);
                                                    }
                                                    match use_music_categories::update_category(
                                                        &cid, None, Some(next),
                                                    ).await {
                                                        Ok(updated) => {
                                                            let mut cats = categories.peek().clone();
                                                            if let Some(c) = cats.iter_mut().find(|c| c.id == updated.id) {
                                                                *c = updated;
                                                            }
                                                            categories.set(cats);
                                                            let next = dirty.peek().0 + 1;
                                                            dirty.set(MusicCategoryDirtyTick(next));
                                                            on_changed.call(());
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                        }
                                        span { class: "label-text", "{cat_name}" }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "divider my-2" }

                    div { class: "flex gap-2",
                        input {
                            class: "input input-bordered input-sm flex-1",
                            placeholder: "{prefill}",
                            value: "{new_name}",
                            oninput: move |e| new_name.set(e.value()),
                        }
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: creating() || new_name().trim().is_empty(),
                            onclick: move |_| {
                                let raw = new_name().trim().to_string();
                                let name = if raw.is_empty() { folder_name.clone() } else { raw };
                                if name.is_empty() { return; }
                                creating.set(true);
                                error.set(None);
                                let folder_id = folder_id_create.clone();
                                spawn(async move {
                                    match use_music_categories::create_category(
                                        &name,
                                        vec![folder_id],
                                    ).await {
                                        Ok(cat) => {
                                            let mut cats = categories.peek().clone();
                                            cats.push(cat);
                                            cats.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                                            categories.set(cats);
                                            new_name.set(String::new());
                                            let next = dirty.peek().0 + 1;
                                            dirty.set(MusicCategoryDirtyTick(next));
                                            on_changed.call(());
                                        }
                                        Err(e) => {
                                            if e == "CONFLICT" {
                                                error.set(Some(format!(
                                                    "A category named \"{}\" already exists", name
                                                )));
                                            } else {
                                                error.set(Some(e));
                                            }
                                        }
                                    }
                                    creating.set(false);
                                });
                            },
                            "Add"
                        }
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn btn-sm",
                        onclick: move |_| on_close.call(()),
                        "Done"
                    }
                }
            }
            div {
                class: "modal-backdrop",
                onclick: move |_| on_close.call(()),
            }
        }
    }
}
