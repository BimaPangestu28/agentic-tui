//! The workspace picker and onboarding scan wizard. This is the landing
//! view (route `/`): it lists the workspaces already configured on the
//! server, and offers a small wizard to scan a folder for repos and save the
//! ones the user checks.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use shared::{RepoDto, WorkspaceDto};

use crate::api;

/// The last path component of `root`, used to name a scanned group until the
/// onboarding grouping UI (Task 6) lets the user name it.
fn root_basename(root: &str) -> String {
    root.trim_end_matches('/')
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(root)
        .to_string()
}

#[component]
pub fn Workspaces() -> impl IntoView {
    let workspaces = RwSignal::new(Vec::<WorkspaceDto>::new());
    let load_error = RwSignal::new(None::<String>);

    let root_input = RwSignal::new(String::new());
    let scan_results = RwSignal::new(Vec::<RepoDto>::new());
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
        let selected: Vec<RepoDto> = scan_results
            .get()
            .into_iter()
            .filter(|repo| checked.contains(&repo.path))
            .collect();
        if selected.is_empty() {
            save_error.set(Some("Check at least one repo before saving.".to_string()));
            return;
        }
        // For now the checked repos become one group named after the scanned
        // folder. Task 6 adds the real grouping-and-naming UI.
        let group = WorkspaceDto {
            name: root_basename(&root_input.get()),
            repos: selected,
        };
        saving.set(true);
        save_error.set(None);
        spawn_local(async move {
            match api::save(std::slice::from_ref(&group)).await {
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
            <div class="page-head">
                <h1>"Workspaces"</h1>
                <p class="sub">"Pick a repository to run against, or add new ones."</p>
            </div>

            {move || {
                load_error
                    .get()
                    .map(|err| view! { <p class="error">{err}</p> })
            }}

            <ul class="workspace-list">
                <For
                    each=move || workspaces.get()
                    key=|workspace| workspace.name.clone()
                    children=move |workspace: WorkspaceDto| {
                        let href = format!("/run/new?workspace={}", workspace.name);
                        let repo_count = workspace.repos.len();
                        let summary = match workspace.repos.first() {
                            Some(first) if repo_count == 1 => first.path.clone(),
                            Some(_) => format!("{repo_count} repos"),
                            None => "no repos".to_string(),
                        };
                        view! {
                            <li class="workspace-row">
                                <A href=href>
                                    <span class="workspace-name">{workspace.name.clone()}</span>
                                    <span class="workspace-path">{summary}</span>
                                </A>
                            </li>
                        }
                    }
                />
            </ul>

            <div class="add-workspace-panel">
                <h2>"Add workspace"</h2>
                <p class="hint">
                    "Point at a folder and scan for git repositories inside it."
                </p>
                <div class="scan-row">
                    <input
                        type="text"
                        class="mono"
                        placeholder="/path/to/projects"
                        prop:value=move || root_input.get()
                        on:input=move |ev| {
                            root_input.set(event_target_value(&ev));
                        }
                    />
                    <button
                        type="button"
                        class="btn-primary"
                        disabled=move || scanning.get()
                        on:click=on_scan
                    >
                        {move || if scanning.get() { "Scanning..." } else { "Scan" }}
                    </button>
                </div>

                {move || {
                    scan_error.get().map(|err| view! { <p class="error">{err}</p> })
                }}

                {move || {
                    (!scan_results.get().is_empty())
                        .then(|| {
                            view! {
                                <div class="scan-results">
                                    <div class="scan-results-head">
                                        <span>
                                            {move || {
                                                format!("{} repositories found", scan_results.get().len())
                                            }}
                                        </span>
                                    </div>
                                    <For
                                        each=move || scan_results.get()
                                        key=|repo| repo.path.clone()
                                        children=move |repo: RepoDto| {
                                            let path_for_checked = repo.path.clone();
                                            let path_for_toggle = repo.path.clone();
                                            view! {
                                                <label class="scan-result-row">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || {
                                                            checked_paths.get().contains(&path_for_checked)
                                                        }
                                                        on:change=move |_| {
                                                            toggle_checked(path_for_toggle.clone());
                                                        }
                                                    />
                                                    <span class="info">
                                                        <span class="workspace-name">{repo.name.clone()}</span>
                                                        <span class="workspace-path">{repo.path.clone()}</span>
                                                    </span>
                                                </label>
                                            }
                                        }
                                    />

                                    {move || {
                                        save_error.get().map(|err| view! { <p class="error">{err}</p> })
                                    }}

                                    <div class="new-run-actions">
                                        <button
                                            type="button"
                                            class="btn-primary"
                                            disabled=move || saving.get()
                                            on:click=on_save
                                        >
                                            {move || if saving.get() { "Saving..." } else { "Save" }}
                                        </button>
                                    </div>
                                </div>
                            }
                        })
                }}
            </div>
        </div>
    }
}
