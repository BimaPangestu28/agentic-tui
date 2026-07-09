//! The workspace picker and onboarding scan wizard. This is the landing
//! view (route `/`): it lists the workspaces already configured on the
//! server, and offers a small wizard to scan a folder for repos and save the
//! ones the user checks.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use shared::WorkspaceDto;

use crate::api;

#[component]
pub fn Workspaces() -> impl IntoView {
    let workspaces = RwSignal::new(Vec::<WorkspaceDto>::new());
    let load_error = RwSignal::new(None::<String>);

    // The add-workspace panel starts collapsed; it is forced open once the
    // initial load comes back empty, and can otherwise be toggled by hand.
    let panel_expanded = RwSignal::new(false);

    let root_input = RwSignal::new(String::new());
    let scan_results = RwSignal::new(Vec::<WorkspaceDto>::new());
    let checked_paths = RwSignal::new(HashSet::<String>::new());
    let scanning = RwSignal::new(false);
    let scan_error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(None::<String>);

    // Loads (or reloads) the workspace list from the server. Called once on
    // mount and again after a successful save.
    let reload_workspaces = move || {
        spawn_local(async move {
            match api::list_workspaces().await {
                Ok(list) => {
                    if list.is_empty() {
                        panel_expanded.set(true);
                    }
                    workspaces.set(list);
                    load_error.set(None);
                }
                Err(err) => load_error.set(Some(err)),
            }
        });
    };

    // Kick off the initial load when the component is created. This app is
    // CSR-only, so component creation and "on mount" happen at the same
    // point.
    reload_workspaces();

    let on_scan = move |_| {
        let root = root_input.get();
        if root.trim().is_empty() {
            scan_error.set(Some("Enter a folder path before scanning.".to_string()));
            return;
        }
        scanning.set(true);
        scan_error.set(None);
        spawn_local(async move {
            match api::scan(&root).await {
                Ok(response) => {
                    // Pre-check every discovered repo; the user unchecks the
                    // ones they do not want to save.
                    checked_paths.set(response.repos.iter().map(|r| r.path.clone()).collect());
                    scan_results.set(response.repos);
                    scan_error.set(None);
                }
                Err(err) => scan_error.set(Some(err)),
            }
            scanning.set(false);
        });
    };

    let on_save = move |_| {
        let checked = checked_paths.get();
        let selected: Vec<WorkspaceDto> = scan_results
            .get()
            .into_iter()
            .filter(|repo| checked.contains(&repo.path))
            .collect();
        if selected.is_empty() {
            save_error.set(Some("Check at least one repo before saving.".to_string()));
            return;
        }
        saving.set(true);
        save_error.set(None);
        spawn_local(async move {
            match api::save(&selected).await {
                Ok(()) => {
                    scan_results.set(Vec::new());
                    checked_paths.set(HashSet::new());
                    root_input.set(String::new());
                    save_error.set(None);
                    reload_workspaces();
                }
                Err(err) => save_error.set(Some(err)),
            }
            saving.set(false);
        });
    };

    let toggle_checked = move |path: String| {
        checked_paths.update(|set| {
            if !set.remove(&path) {
                set.insert(path);
            }
        });
    };

    view! {
        <div class="workspaces-view">
            <h1>"Workspaces"</h1>

            {move || {
                load_error
                    .get()
                    .map(|err| view! { <p class="error">{err}</p> })
            }}

            <ul class="workspace-list">
                <For
                    each=move || workspaces.get()
                    key=|workspace| workspace.path.clone()
                    children=move |workspace: WorkspaceDto| {
                        let href = format!("/run/new?workspace={}", workspace.name);
                        view! {
                            <li class="workspace-row">
                                <A href=href>
                                    <span class="workspace-name">{workspace.name.clone()}</span>
                                    <span class="workspace-path">{workspace.path.clone()}</span>
                                </A>
                            </li>
                        }
                    }
                />
            </ul>

            <button
                type="button"
                on:click=move |_| panel_expanded.update(|expanded| *expanded = !*expanded)
            >
                {move || {
                    if panel_expanded.get() {
                        "Hide add workspace"
                    } else {
                        "Add workspace"
                    }
                }}
            </button>

            {move || {
                panel_expanded
                    .get()
                    .then(|| {
                        view! {
                            <div class="add-workspace-panel">
                                <h2>"Add workspace"</h2>
                                <label>
                                    "Folder to scan"
                                    <input
                                        type="text"
                                        placeholder="/path/to/projects"
                                        prop:value=move || root_input.get()
                                        on:input=move |ev| {
                                            root_input.set(event_target_value(&ev));
                                        }
                                    />
                                </label>
                                <button
                                    type="button"
                                    disabled=move || scanning.get()
                                    on:click=on_scan
                                >
                                    {move || if scanning.get() { "Scanning..." } else { "Scan" }}
                                </button>

                                {move || {
                                    scan_error
                                        .get()
                                        .map(|err| view! { <p class="error">{err}</p> })
                                }}

                                <ul class="scan-results">
                                    <For
                                        each=move || scan_results.get()
                                        key=|repo| repo.path.clone()
                                        children=move |repo: WorkspaceDto| {
                                            let path_for_checked = repo.path.clone();
                                            let path_for_toggle = repo.path.clone();
                                            view! {
                                                <li class="scan-result-row">
                                                    <label>
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || {
                                                                checked_paths.get().contains(&path_for_checked)
                                                            }
                                                            on:change=move |_| {
                                                                toggle_checked(path_for_toggle.clone());
                                                            }
                                                        />
                                                        {repo.name.clone()}
                                                        " "
                                                        {repo.path.clone()}
                                                    </label>
                                                </li>
                                            }
                                        }
                                    />
                                </ul>

                                {move || {
                                    save_error
                                        .get()
                                        .map(|err| view! { <p class="error">{err}</p> })
                                }}

                                <button
                                    type="button"
                                    disabled=move || saving.get() || scan_results.get().is_empty()
                                    on:click=on_save
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </button>
                            </div>
                        }
                    })
            }}
        </div>
    }
}
