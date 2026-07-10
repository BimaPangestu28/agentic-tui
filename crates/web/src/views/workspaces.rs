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

/// The last path component of `root`, used to prefill the group-name input
/// once a scan of that root comes back.
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
    let group_name = RwSignal::new(String::new());
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
                    // Prefill the group name from the scanned root so the
                    // common case (accept the default) needs no typing.
                    group_name.set(root_basename(&root));
                    scan_results.set(response.repos);
                    scan_error.set(None);
                }
                Err(err) => scan_error.set(Some(err)),
            }
            scanning.set(false);
        });
    };

    let on_save = move |_| {
        let name = group_name.get().trim().to_string();
        let checked = checked_paths.get();
        let selected: Vec<RepoDto> = scan_results
            .get()
            .into_iter()
            .filter(|repo| checked.contains(&repo.path))
            .collect();
        match (name.is_empty(), selected.is_empty()) {
            (true, true) => {
                save_error.set(Some(
                    "Enter a workspace name and check at least one repo before saving.".to_string(),
                ));
                return;
            }
            (true, false) => {
                save_error.set(Some("Enter a workspace name before saving.".to_string()));
                return;
            }
            (false, true) => {
                save_error.set(Some("Check at least one repo before saving.".to_string()));
                return;
            }
            (false, false) => {}
        }
        // All the checked repos become one group under the name the user
        // gave it.
        let group = WorkspaceDto {
            name,
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
                    group_name.set(String::new());
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

    // Shared utility strings for elements that repeat the same look across the
    // view (mirrors the `nav_link` pattern in `components.rs`). All are
    // `&'static str`, so the `move` closures below capture them by copy.
    let error_class = "flex items-start gap-2 rounded-md border border-block/30 \
                       bg-block/10 px-4 py-3 text-[13px] text-danger \
                       before:content-['\u{26a0}']";
    let mono_input = "w-full rounded-md border border-line bg-inset px-[14px] py-2.5 \
                      min-h-[38px] font-mono text-[13px] text-ink transition-colors \
                      placeholder:text-dim hover:border-line-strong focus:outline-none \
                      focus:border-accent/40 focus:bg-surface focus:ring-[3px] \
                      focus:ring-accent/12";
    let btn_primary = "inline-flex items-center justify-center gap-2 rounded-md \
                       bg-accent px-[18px] py-2.5 min-h-[38px] text-[14px] font-semibold \
                       text-accent-fg shadow-card transition-colors hover:bg-accent-hover \
                       active:bg-accent-press disabled:opacity-50 disabled:cursor-not-allowed";

    view! {
        <div class="flex flex-col gap-8">
            <div class="mb-6">
                <h1 class="text-[28px] font-semibold tracking-tight">"Workspaces"</h1>
                <p class="mt-2 text-[15px] text-muted">
                    "Pick a repository to run against, or add new ones."
                </p>
            </div>

            {move || {
                load_error
                    .get()
                    .map(|err| view! { <p class=error_class>{err}</p> })
            }}

            <ul class="grid grid-cols-1 gap-3 min-[560px]:grid-cols-2 min-[900px]:grid-cols-4 empty:hidden">
                <For
                    each=move || workspaces.get()
                    key=|workspace| workspace.name.clone()
                    children=move |workspace: WorkspaceDto| {
                        let href = format!("/run/new?workspace={}", workspace.name);
                        let repo_count = workspace.repos.len();
                        let summary = match repo_count {
                            0 => "no repos".to_string(),
                            // A single-repo group still shows its path; the
                            // repo count is what every group needs, though.
                            1 => format!("{} · 1 repo", workspace.repos[0].path),
                            n => format!("{n} repos"),
                        };
                        view! {
                            <li class="flex">
                                <A
                                    attr:class="flex w-full flex-col gap-2 rounded-md border \
                                                border-line bg-surface p-4 text-inherit \
                                                no-underline cursor-pointer transition-colors \
                                                hover:border-accent/40 hover:bg-inset \
                                                focus-visible:outline-none focus-visible:ring-2 \
                                                focus-visible:ring-accent focus-visible:ring-offset-2 \
                                                focus-visible:ring-offset-page"
                                    href=href
                                >
                                    <span class="flex items-center gap-2">
                                        <span class="shrink-0 text-[15px] text-accent opacity-[0.85]">"\u{2b21}"</span>
                                        <span class="text-[15px] font-semibold text-ink">{workspace.name.clone()}</span>
                                    </span>
                                    <span class="break-all font-mono text-[13px] text-dim">{summary}</span>
                                </A>
                            </li>
                        }
                    }
                />
            </ul>

            <div class="flex flex-col gap-4 rounded-lg border border-line bg-surface p-6 shadow-card">
                <h2 class="text-[18px] font-semibold tracking-tight">"Add workspace"</h2>
                <p class="text-[13px] leading-normal text-dim">
                    "Point at a folder and scan for git repositories inside it."
                </p>
                <div class="flex flex-col items-stretch gap-3 min-[560px]:flex-row">
                    <input
                        type="text"
                        class=mono_input
                        placeholder="/path/to/projects"
                        prop:value=move || root_input.get()
                        on:input=move |ev| {
                            root_input.set(event_target_value(&ev));
                        }
                    />
                    <button
                        type="button"
                        class=format!("{btn_primary} whitespace-nowrap")
                        disabled=move || scanning.get()
                        on:click=on_scan
                    >
                        {move || if scanning.get() { "Scanning..." } else { "Scan" }}
                    </button>
                </div>

                {move || {
                    scan_error.get().map(|err| view! { <p class=error_class>{err}</p> })
                }}

                {move || {
                    (!scan_results.get().is_empty())
                        .then(|| {
                            view! {
                                <div class="flex flex-col gap-1 border-t border-line pt-4">
                                    <div class="mb-1 flex items-center justify-between text-[13px] text-muted">
                                        <span>
                                            {move || {
                                                format!("{} repositories found", scan_results.get().len())
                                            }}
                                        </span>
                                    </div>
                                    <div class="flex flex-col gap-2">
                                        <label class="text-[13px] font-semibold text-ink">"Workspace name"</label>
                                        <input
                                            type="text"
                                            class=mono_input
                                            placeholder="e.g. platform-repos"
                                            prop:value=move || group_name.get()
                                            on:input=move |ev| {
                                                group_name.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>

                                    <For
                                        each=move || scan_results.get()
                                        key=|repo| repo.path.clone()
                                        children=move |repo: RepoDto| {
                                            let path_for_checked = repo.path.clone();
                                            let path_for_toggle = repo.path.clone();
                                            view! {
                                                <label class="flex items-center gap-3 rounded-md p-3 transition-colors hover:bg-inset">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || {
                                                            checked_paths.get().contains(&path_for_checked)
                                                        }
                                                        on:change=move |_| {
                                                            toggle_checked(path_for_toggle.clone());
                                                        }
                                                    />
                                                    <span class="flex min-w-0 flex-col gap-px">
                                                        <span class="text-[14px] font-semibold text-ink">{repo.name.clone()}</span>
                                                        <span class="truncate font-mono text-[13px] text-dim">{repo.path.clone()}</span>
                                                    </span>
                                                </label>
                                            }
                                        }
                                    />

                                    {move || {
                                        save_error.get().map(|err| view! { <p class=error_class>{err}</p> })
                                    }}

                                    <div class="flex items-center justify-end gap-3">
                                        <button
                                            type="button"
                                            class=btn_primary
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
