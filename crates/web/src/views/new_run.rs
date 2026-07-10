//! The "new run" form (route `/run/new`): collects the goal and options for
//! the selected workspace, optionally runs the goal-refine clarification
//! flow against the server, then starts the pipeline run and navigates to
//! its dashboard.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;
use shared::{Language, RepoDto, StartRunRequest, WorkspaceDto};

use crate::api;

/// The fallback integration branch name offered when a repo's workspace config
/// does not pin one. Matches the server's default in `run::start`.
const DEFAULT_INTEGRATION: &str = "agentic-integration";

/// Where a repo's branch list is in its fetch lifecycle. `Loaded` drives the
/// real dropdowns; `Failed` falls the form back to free-text inputs so a repo
/// whose branches could not be listed never blocks starting a run.
#[derive(Clone, Debug, PartialEq)]
enum BranchStatus {
    Loading,
    Loaded,
    Failed,
}

/// Per-repo branch selection for the form: the chosen base ref and integration
/// branch, plus the repo's real branches once fetched. Held as plain data in a
/// single `Vec` signal (updated by index), not a signal per field.
#[derive(Clone, Debug, PartialEq)]
struct RepoBranchState {
    name: String,
    path: String,
    base: String,
    integration: String,
    branches: Vec<String>,
    status: BranchStatus,
}

/// Build the `WorkspaceDto` to submit, folding each repo's chosen base and
/// integration branch (blank means "unset", so the server applies its default)
/// back into the repos. Returns `None` if the workspace has not loaded yet.
fn workspace_from_states(name: &str, states: &[RepoBranchState]) -> WorkspaceDto {
    let repos = states
        .iter()
        .map(|state| RepoDto {
            name: state.name.clone(),
            path: state.path.clone(),
            base: normalize(state.base.clone()),
            integration: normalize(state.integration.clone()),
        })
        .collect();
    WorkspaceDto {
        name: name.to_string(),
        repos,
    }
}

/// Local state for the refine sub-flow, driven entirely by user actions and
/// server responses. `Editing` is the initial form. `Answering` holds the
/// pass-1 clarifying questions together with the answers collected so far.
/// `Confirming` shows the final goal, editable, before the run actually
/// starts. `Submitting` disables the form while a network call is in
/// flight (refine or start). `Error` surfaces a message inline; the form
/// inputs themselves live in separate signals, so nothing entered is lost
/// when this state is reached.
#[derive(Clone, Debug, PartialEq)]
enum FlowState {
    Editing,
    Answering {
        questions: Vec<String>,
        answers: Vec<String>,
        refined_goal: String,
        cost: f64,
    },
    Confirming {
        goal: String,
        cost: f64,
    },
    Submitting,
    Error(String),
}

/// The directory the refine passes run in. The refine passes launch a single
/// `claude` process against one working directory, not one per repo, so a
/// multi-repo workspace needs a single shared root. A one-repo workspace has
/// nothing to share a root with, so its parent directory is used instead. For
/// several repos, this walks each path's `/`-separated components and keeps
/// the leading run that is identical across all of them; if that run is
/// empty (or only the leading `/`), the repos have no meaningful common
/// ancestor (e.g. they live under unrelated trees), so the first repo's own
/// path is used rather than refining against the filesystem root.
fn common_root(repos: &[RepoDto]) -> String {
    let Some(first) = repos.first() else {
        return String::new();
    };
    if repos.len() == 1 {
        return match first.path.rsplit_once('/') {
            Some(("", _)) => "/".to_string(),
            Some((parent, _)) => parent.to_string(),
            None => first.path.clone(),
        };
    }

    let mut common: Vec<&str> = first.path.split('/').collect();
    for repo in &repos[1..] {
        let parts: Vec<&str> = repo.path.split('/').collect();
        let shared = common
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        common.truncate(shared);
    }

    let joined = common.join("/");
    if joined.is_empty() || joined == "/" {
        first.path.clone()
    } else {
        joined
    }
}

/// Fetch each repo's branches in parallel and update its state in place. On
/// success, populate `branches` and default an unset base to the repo's current
/// branch (or its first branch). On failure, mark the repo `Failed` so the form
/// falls back to free-text inputs, defaulting an unset base to `HEAD`.
fn load_branches(repo_states: RwSignal<Vec<RepoBranchState>>) {
    let paths: Vec<(usize, String)> = repo_states
        .get_untracked()
        .iter()
        .enumerate()
        .map(|(i, state)| (i, state.path.clone()))
        .collect();
    for (index, path) in paths {
        spawn_local(async move {
            match api::list_branches(&path).await {
                Ok(response) => repo_states.update(|states| {
                    if let Some(state) = states.get_mut(index) {
                        state.status = BranchStatus::Loaded;
                        if state.base.trim().is_empty() {
                            state.base = response
                                .current
                                .clone()
                                .or_else(|| response.branches.first().cloned())
                                .unwrap_or_default();
                        }
                        state.branches = response.branches;
                    }
                }),
                Err(_) => repo_states.update(|states| {
                    if let Some(state) = states.get_mut(index) {
                        state.status = BranchStatus::Failed;
                        if state.base.trim().is_empty() {
                            state.base = "HEAD".to_string();
                        }
                    }
                }),
            }
        });
    }
}

/// One repo's branch controls: a base-branch dropdown (of the repo's real
/// branches, or a free-text input while loading or after a failed fetch) and an
/// editable integration-branch combobox (a `datalist` of existing branches the
/// user can pick from or type past). Edits write back into `repo_states[index]`.
fn repo_branch_row(
    repo_states: RwSignal<Vec<RepoBranchState>>,
    index: usize,
    state: RepoBranchState,
) -> impl IntoView {
    let control_class = "w-full rounded-md border border-line bg-inset px-[12px] py-2 \
        min-h-[36px] font-mono text-[13px] text-ink transition-colors \
        hover:border-line-strong focus:outline-none focus:border-accent/40 \
        focus:bg-surface focus:ring-[3px] focus:ring-accent/12";
    let sub_label = "text-[12px] font-semibold text-muted";
    let datalist_id = format!("branches-{index}");

    // Base: a real dropdown once branches are known; a free-text input while
    // loading or after a failed fetch (so the form is never blocked).
    let base_control = if matches!(state.status, BranchStatus::Loaded) && !state.branches.is_empty()
    {
        let selected_base = state.base.clone();
        let options = state
            .branches
            .iter()
            .cloned()
            .map(|branch| {
                let is_selected = branch == selected_base;
                let label = branch.clone();
                view! { <option value=branch selected=is_selected>{label}</option> }
            })
            .collect::<Vec<_>>();
        view! {
            <select
                class=control_class
                prop:value=state.base.clone()
                on:change=move |ev| {
                    let value = event_target_value(&ev);
                    repo_states
                        .update(|states| {
                            if let Some(s) = states.get_mut(index) {
                                s.base = value;
                            }
                        });
                }
            >
                {options}
            </select>
        }
        .into_any()
    } else {
        view! {
            <input
                type="text"
                class=control_class
                prop:value=state.base.clone()
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    repo_states
                        .update(|states| {
                            if let Some(s) = states.get_mut(index) {
                                s.base = value;
                            }
                        });
                }
            />
        }
        .into_any()
    };

    // Integration: an editable combobox via a native datalist, so the user can
    // pick an existing branch or type a new name.
    let integration_options = state
        .branches
        .iter()
        .cloned()
        .map(|branch| view! { <option value=branch></option> })
        .collect::<Vec<_>>();

    let status_hint = match state.status {
        BranchStatus::Loading => Some(
            view! { <span class="text-[12px] text-dim">"Loading branches..."</span> }.into_any(),
        ),
        BranchStatus::Failed => Some(
            view! {
                <span class="text-[12px] text-danger">
                    "Could not list branches; enter refs by hand."
                </span>
            }
            .into_any(),
        ),
        BranchStatus::Loaded => None,
    };

    view! {
        <div class="flex flex-col gap-2 rounded-md border border-line bg-surface p-3">
            <div class="flex flex-col gap-px min-w-0">
                <span class="text-[14px] font-semibold text-ink">{state.name.clone()}</span>
                <span class="truncate font-mono text-[12px] text-dim">{state.path.clone()}</span>
            </div>
            {status_hint}
            <div class="grid grid-cols-1 min-[560px]:grid-cols-2 gap-3">
                <div class="flex flex-col gap-1">
                    <label class=sub_label>"Base branch"</label>
                    {base_control}
                </div>
                <div class="flex flex-col gap-1">
                    <label class=sub_label>"Integration branch"</label>
                    <input
                        type="text"
                        class=control_class
                        list=datalist_id.clone()
                        prop:value=state.integration.clone()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            repo_states
                                .update(|states| {
                                    if let Some(s) = states.get_mut(index) {
                                        s.integration = value;
                                    }
                                });
                        }
                    />
                    <datalist id=datalist_id>{integration_options}</datalist>
                </div>
            </div>
        </div>
    }
}

/// Trims a text input and turns a blank value into `None`, matching the
/// "empty means unset" convention `StartRunRequest`'s optional fields use.
fn normalize(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Starts the run and navigates to its dashboard on success, or reports the
/// server's error message (a 400 validation message or a 409 busy message)
/// inline on failure.
async fn launch_run(
    navigate: impl Fn(&str, NavigateOptions) + 'static,
    flow: RwSignal<FlowState>,
    request: StartRunRequest,
) {
    match api::start_run(request).await {
        Ok(response) => {
            navigate(
                &format!("/run/{}", response.run_id),
                NavigateOptions::default(),
            );
        }
        Err(err) => flow.set(FlowState::Error(err)),
    }
}

#[component]
pub fn NewRun() -> impl IntoView {
    let query = use_query_map();
    let navigate = use_navigate();
    let workspace_name = query.get_untracked().get("workspace").unwrap_or_default();

    let workspace = RwSignal::new(None::<WorkspaceDto>);
    let load_error = RwSignal::new(None::<String>);

    let goal_input = RwSignal::new(String::new());
    let verify_input = RwSignal::new(String::new());
    let refine_enabled = RwSignal::new(true);
    let language = RwSignal::new(Language::English);
    // Per-repo base/integration branch selection, filled once the workspace
    // loads and each repo's branches are fetched.
    let repo_states = RwSignal::new(Vec::<RepoBranchState>::new());

    let flow = RwSignal::new(FlowState::Editing);

    // Inline Tailwind utility recipes reused across the form and the refine
    // sub-flow. Named bindings (mirroring the app bar's `nav_link`) keep the
    // repeated input/button/panel looks defined in one place. They are
    // `&'static str`, so each closure below copies rather than moves them.
    let field_class = "flex flex-col gap-2";
    let field_label = "text-[13px] font-semibold text-ink";
    let hint_class = "text-[13px] leading-normal text-dim";
    let input_class = "w-full rounded-md border border-line bg-inset px-[14px] py-2.5 \
        min-h-[38px] text-[14px] text-ink transition-colors placeholder:text-dim \
        hover:border-line-strong focus:outline-none focus:border-accent/40 \
        focus:bg-surface focus:ring-[3px] focus:ring-accent/12";
    let primary_button = "inline-flex items-center justify-center gap-2 rounded-md \
        bg-accent px-[18px] py-2.5 min-h-[38px] text-[14px] font-semibold \
        text-accent-fg shadow-card transition-colors hover:bg-accent-hover \
        active:bg-accent-press disabled:opacity-50 disabled:cursor-not-allowed";
    let actions_class = "flex items-center justify-end gap-3";
    let error_class = "flex items-start gap-2 rounded-md border border-block/30 \
        bg-block/10 px-4 py-3 text-[13px] text-danger before:content-['⚠']";
    let banner_class = "flex items-center gap-2 rounded-md border border-line \
        bg-surface px-4 py-3 text-[13px] text-muted";
    let spinner_class = "size-[13px] shrink-0 rounded-full border-2 border-line-strong \
        border-t-accent animate-spin";
    let refine_box_class = "relative flex flex-col gap-4 rounded-lg border \
        border-accent/40 bg-surface p-6 shadow-pop before:content-[''] before:absolute \
        before:left-0 before:top-4 before:bottom-4 before:w-[3px] before:rounded-full \
        before:bg-accent";
    let refine_head_class = "flex items-center gap-2 text-[13px] font-bold uppercase \
        tracking-[0.04em] text-accent";
    let refine_step_class = "font-mono rounded-full border border-accent/40 bg-accent/12 \
        px-2 py-0.5 text-[12px]";

    // Load the workspace list once and pick out the one named by the
    // `workspace` query param. This app is CSR-only, so component creation
    // and "on mount" are the same point in time.
    {
        let name = workspace_name.clone();
        spawn_local(async move {
            match api::list_workspaces().await {
                Ok(list) => match list.into_iter().find(|w| w.name == name) {
                    Some(found) => {
                        // Seed the per-repo branch state from the workspace
                        // config (base/integration may be preset there), then
                        // fetch each repo's real branches to populate the
                        // dropdowns.
                        repo_states.set(
                            found
                                .repos
                                .iter()
                                .map(|repo| RepoBranchState {
                                    name: repo.name.clone(),
                                    path: repo.path.clone(),
                                    base: repo.base.clone().unwrap_or_default(),
                                    integration: repo
                                        .integration
                                        .clone()
                                        .unwrap_or_else(|| DEFAULT_INTEGRATION.to_string()),
                                    branches: Vec::new(),
                                    status: BranchStatus::Loading,
                                })
                                .collect(),
                        );
                        workspace.set(Some(found));
                        load_branches(repo_states);
                    }
                    None => {
                        load_error.set(Some(format!("workspace '{name}' was not found")));
                    }
                },
                Err(err) => load_error.set(Some(err)),
            }
        });
    }

    // Submit from the form. With refine on, this kicks off pass 1; with
    // refine off, it starts the run directly with a zero refine cost.
    let navigate_for_start = navigate.clone();
    let on_start = move |_| {
        let goal = goal_input.get_untracked().trim().to_string();
        if goal.is_empty() {
            flow.set(FlowState::Error(
                "Enter a goal before starting.".to_string(),
            ));
            return;
        }
        let Some(selected) = workspace.get_untracked() else {
            flow.set(FlowState::Error(
                "The workspace is still loading.".to_string(),
            ));
            return;
        };

        if refine_enabled.get_untracked() {
            flow.set(FlowState::Submitting);
            let root = common_root(&selected.repos);
            let lang = language.get_untracked();
            spawn_local(async move {
                match api::refine_questions(&root, &goal, lang).await {
                    Ok(response) if response.questions.is_empty() => {
                        flow.set(FlowState::Confirming {
                            goal: response.refined_goal,
                            cost: response.cost,
                        });
                    }
                    Ok(response) => {
                        let answers = vec![String::new(); response.questions.len()];
                        flow.set(FlowState::Answering {
                            questions: response.questions,
                            answers,
                            refined_goal: response.refined_goal,
                            cost: response.cost,
                        });
                    }
                    Err(err) => flow.set(FlowState::Error(err)),
                }
            });
        } else {
            let verify = normalize(verify_input.get_untracked());
            flow.set(FlowState::Submitting);
            let navigate = navigate_for_start.clone();
            let request = StartRunRequest {
                workspace: workspace_from_states(&selected.name, &repo_states.get_untracked()),
                goal,
                verify,
                refine_cost: 0.0,
                language: language.get_untracked(),
            };
            spawn_local(async move {
                launch_run(navigate, flow, request).await;
            });
        }
    };

    // "Continue" from the Answering state: folds the collected answers into
    // pass 2 and moves to Confirming with the accumulated cost.
    let on_continue = move |_| {
        let (questions, answers, cost) = match flow.get_untracked() {
            FlowState::Answering {
                questions,
                answers,
                cost,
                ..
            } => (questions, answers, cost),
            _ => return,
        };
        let Some(selected) = workspace.get_untracked() else {
            flow.set(FlowState::Error(
                "The workspace is still loading.".to_string(),
            ));
            return;
        };
        let original_goal = goal_input.get_untracked().trim().to_string();
        let qa_pairs: Vec<(String, String)> = questions.into_iter().zip(answers).collect();
        let root = common_root(&selected.repos);
        let lang = language.get_untracked();
        flow.set(FlowState::Submitting);
        spawn_local(async move {
            match api::refine_finalize(&root, &original_goal, qa_pairs, lang).await {
                Ok(response) => flow.set(FlowState::Confirming {
                    goal: response.refined_goal,
                    cost: cost + response.cost,
                }),
                Err(err) => flow.set(FlowState::Error(err)),
            }
        });
    };

    // "Plan" from the Confirming state: starts the run with the (possibly
    // edited) final goal and the accumulated refine cost.
    let navigate_for_plan = navigate.clone();
    let on_plan = move |_| {
        let (goal, cost) = match flow.get_untracked() {
            FlowState::Confirming { goal, cost } => (goal, cost),
            _ => return,
        };
        let Some(selected) = workspace.get_untracked() else {
            flow.set(FlowState::Error(
                "The workspace is still loading.".to_string(),
            ));
            return;
        };
        let verify = normalize(verify_input.get_untracked());
        flow.set(FlowState::Submitting);
        let navigate = navigate_for_plan.clone();
        let request = StartRunRequest {
            workspace: workspace_from_states(&selected.name, &repo_states.get_untracked()),
            goal,
            verify,
            refine_cost: cost,
            language: language.get_untracked(),
        };
        spawn_local(async move {
            launch_run(navigate, flow, request).await;
        });
    };

    // The goal/options form, shared by the `Editing` and `Error` states so
    // an error never discards what was typed. Callable more than once (the
    // enclosing view re-renders on every flow change), so the click handler
    // is cloned rather than moved out on each call.
    let render_form = move |error: Option<String>| {
        view! {
            <div class="flex flex-col gap-6">
                {error.map(|msg| view! { <p class=error_class>{msg}</p> })}
                <div class=field_class>
                    <label class="text-[14px] font-semibold text-ink">"Goal"</label>
                    <textarea
                        class="w-full rounded-md border border-line-strong bg-surface p-4 \
                            min-h-[148px] text-[15px] leading-[1.55] text-ink resize-y \
                            transition-colors placeholder:text-dim hover:border-line-strong \
                            focus:outline-none focus:border-accent/40 focus:bg-surface \
                            focus:ring-[3px] focus:ring-accent/12"
                        rows="6"
                        placeholder="Describe what you want built..."
                        prop:value=move || goal_input.get()
                        on:input=move |ev| goal_input.set(event_target_value(&ev))
                    ></textarea>
                    <p class=hint_class>
                        "Claude Code breaks this into epics, then runs each one in its own git worktree."
                    </p>
                </div>

                <div class=field_class>
                    <label class=field_label>"Repos in scope"</label>
                    <p class=hint_class>
                        "For each repo, pick the branch to build from and the branch the epics merge into."
                    </p>
                    <div class="flex flex-col gap-3 rounded-md border border-line p-3 max-h-[360px] overflow-y-auto">
                        {move || {
                            repo_states
                                .get()
                                .into_iter()
                                .enumerate()
                                .map(|(index, state)| repo_branch_row(repo_states, index, state))
                                .collect::<Vec<_>>()
                        }}
                    </div>
                </div>

                <div class="grid grid-cols-1 min-[900px]:grid-cols-2 gap-4 rounded-lg border border-line bg-surface p-6">
                    <div class="col-span-full text-[12px] font-bold uppercase tracking-[0.06em] text-dim">"Advanced options"</div>
                    <div class="flex flex-col gap-2 col-span-full">
                        <label class=field_label>"Default verify command"</label>
                        <input
                            type="text"
                            class="w-full rounded-md border border-line bg-inset px-[14px] py-2.5 \
                                min-h-[38px] font-mono text-[13px] text-ink transition-colors \
                                placeholder:text-dim hover:border-line-strong focus:outline-none \
                                focus:border-accent/40 focus:bg-surface focus:ring-[3px] \
                                focus:ring-accent/12"
                            placeholder="make verify"
                            prop:value=move || verify_input.get()
                            on:input=move |ev| verify_input.set(event_target_value(&ev))
                        />
                        <p class=hint_class>
                            "The planner may choose a verify command per repo; this is the fallback."
                        </p>
                    </div>
                    <div class="flex flex-col gap-2 col-span-full">
                        <label class=field_label>"Language"</label>
                        <select
                            class="w-full rounded-md border border-line bg-inset px-[14px] py-2.5 \
                                min-h-[38px] text-[14px] text-ink transition-colors \
                                hover:border-line-strong focus:outline-none focus:border-accent/40 \
                                focus:bg-surface focus:ring-[3px] focus:ring-accent/12"
                            prop:value=move || {
                                match language.get() {
                                    Language::English => "english",
                                    Language::Indonesian => "indonesian",
                                }
                            }
                            on:change=move |ev| {
                                let value = event_target_value(&ev);
                                language
                                    .set(
                                        if value == "indonesian" {
                                            Language::Indonesian
                                        } else {
                                            Language::English
                                        },
                                    );
                            }
                        >
                            <option value="english">"English"</option>
                            <option value="indonesian">"Indonesia"</option>
                        </select>
                        <p class=hint_class>
                            "Language for clarifying questions, summaries, and prose. Code, comments, and commit messages stay English."
                        </p>
                    </div>
                </div>

                <label class="flex flex-row items-center gap-3 rounded-md border border-line bg-surface px-4 py-3 cursor-pointer text-[13px] font-medium text-ink">
                    <input
                        type="checkbox"
                        prop:checked=move || refine_enabled.get()
                        on:change=move |_| refine_enabled.update(|value| *value = !*value)
                    />
                    "Refine before planning"
                    <span class="ml-auto text-[13px] leading-normal text-dim">"Ask clarifying questions first"</span>
                </label>

                <div class=actions_class>
                    <button
                        type="button"
                        class="inline-flex items-center justify-center gap-2 rounded-md border border-transparent bg-transparent px-[18px] py-2.5 min-h-[38px] text-[14px] font-medium text-muted transition-colors hover:bg-inset hover:text-ink"
                        on:click={
                            let nav = navigate.clone();
                            move |_| nav("/", Default::default())
                        }
                    >
                        "Cancel"
                    </button>
                    <button type="button" class=primary_button on:click=on_start.clone()>
                        {move || {
                            if refine_enabled.get() { "Refine & plan" } else { "Start run" }
                        }}
                    </button>
                </div>
            </div>
        }
    };

    view! {
        <div class="mx-auto flex max-w-[720px] flex-col gap-6">
            <div class="mb-6">
                <h1 class="text-[28px] font-semibold tracking-tight">"New run"</h1>
                {move || {
                    workspace
                        .get()
                        .map(|ws| {
                            let repo_count = ws.repos.len();
                            let repo_summary = if repo_count == 1 {
                                ws.repos
                                    .first()
                                    .map(|r| r.path.clone())
                                    .unwrap_or_default()
                            } else {
                                format!("{repo_count} repos")
                            };
                            view! {
                                <p class="text-[15px] text-muted mt-2">
                                    "Workspace " <strong>{ws.name.clone()}</strong> " \u{00b7} "
                                    <span class="font-mono">{repo_summary}</span>
                                </p>
                            }
                        })
                }}
            </div>
            {move || {
                if let Some(err) = load_error.get() {
                    view! { <p class=error_class>{err}</p> }.into_any()
                } else if workspace.get().is_none() {
                    view! {
                        <div class=banner_class>
                            <span class=spinner_class></span>
                            "Loading workspace..."
                        </div>
                    }
                        .into_any()
                } else {
                    match flow.get() {
                        FlowState::Editing => render_form(None).into_any(),
                        FlowState::Error(msg) => render_form(Some(msg)).into_any(),
                        FlowState::Submitting => {
                            view! {
                                <div class=banner_class>
                                    <span class=spinner_class></span>
                                    "Starting the run..."
                                </div>
                            }
                                .into_any()
                        }
                        FlowState::Answering { questions, answers, .. } => {
                            let rows: Vec<_> = questions
                                .iter()
                                .enumerate()
                                .map(|(i, question)| {
                                    let question = question.clone();
                                    let current_answer =
                                        answers.get(i).cloned().unwrap_or_default();
                                    view! {
                                        <div class="flex flex-col gap-2 pb-4 border-b border-line last-of-type:border-b-0 last-of-type:pb-0">
                                            <div class="flex gap-2 font-medium text-ink before:content-['?'] before:shrink-0 before:size-5 before:grid before:place-content-center before:rounded-full before:bg-accent/12 before:text-accent before:font-bold before:text-[12px]">{question}</div>
                                            <input
                                                type="text"
                                                class=input_class
                                                placeholder="Your answer..."
                                                prop:value=current_answer
                                                on:input=move |ev| {
                                                    let value = event_target_value(&ev);
                                                    flow.update(|f| {
                                                        if let FlowState::Answering {
                                                            answers,
                                                            ..
                                                        } = f
                                                        {
                                                            if let Some(slot) = answers.get_mut(i) {
                                                                *slot = value;
                                                            }
                                                        }
                                                    });
                                                }
                                            />
                                        </div>
                                    }
                                })
                                .collect();
                            view! {
                                <div class=refine_box_class>
                                    <div class=refine_head_class>
                                        <span class=refine_step_class>"Step 1 / 2"</span>
                                        "Answer a few questions"
                                    </div>
                                    {rows}
                                    <div class=actions_class>
                                        <button
                                            type="button"
                                            class=primary_button
                                            on:click=on_continue
                                        >
                                            "Continue"
                                        </button>
                                    </div>
                                </div>
                            }
                            .into_any()
                        }
                        FlowState::Confirming { goal, .. } => {
                            view! {
                                <div class=refine_box_class>
                                    <div class=refine_head_class>
                                        <span class=refine_step_class>"Step 2 / 2"</span>
                                        "Confirm the refined goal"
                                    </div>
                                    <p class=hint_class>
                                        "Edit as needed, then accept to begin planning."
                                    </p>
                                    <div class=field_class>
                                        <textarea
                                            class="w-full rounded-md border border-line bg-inset px-[14px] py-2.5 \
                                                min-h-[110px] text-[15px] leading-[1.55] text-ink resize-y \
                                                transition-colors placeholder:text-dim hover:border-line-strong \
                                                focus:outline-none focus:border-accent/40 focus:bg-surface \
                                                focus:ring-[3px] focus:ring-accent/12"
                                            rows="6"
                                            prop:value=goal
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                flow.update(|f| {
                                                    if let FlowState::Confirming { goal, .. } = f {
                                                        *goal = value;
                                                    }
                                                });
                                            }
                                        ></textarea>
                                    </div>
                                    <div class=actions_class>
                                        <button
                                            type="button"
                                            class=primary_button
                                            on:click=on_plan.clone()
                                        >
                                            "Accept & plan"
                                        </button>
                                    </div>
                                </div>
                            }
                            .into_any()
                        }
                    }
                }
            }}
        </div>
    }
}
