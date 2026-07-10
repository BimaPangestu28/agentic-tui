//! The "new run" form (route `/run/new`): collects the goal and options for
//! the selected workspace, optionally runs the goal-refine clarification
//! flow against the server, then starts the pipeline run and navigates to
//! its dashboard.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;
use shared::{RepoDto, StartRunRequest, WorkspaceDto};

use crate::api;

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
    let primary_button = "btn-primary";
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
                    Some(found) => workspace.set(Some(found)),
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
            spawn_local(async move {
                match api::refine_questions(&root, &goal).await {
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
                workspace: selected,
                goal,
                verify,
                refine_cost: 0.0,
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
        flow.set(FlowState::Submitting);
        spawn_local(async move {
            match api::refine_finalize(&root, &original_goal, qa_pairs).await {
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
            workspace: selected,
            goal,
            verify,
            refine_cost: cost,
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
        // The workspace is loaded before the form renders (the Editing and Error
        // states are only reached after the load completes), so read it
        // untracked: the repo list is fixed for the lifetime of this view.
        let repo_rows: Vec<_> = workspace
            .get_untracked()
            .map(|ws| {
                ws.repos
                    .into_iter()
                    .map(|repo| {
                        view! {
                            <div class="flex flex-col gap-px min-w-0 px-3 py-2">
                                <span class="text-[15px] font-semibold text-ink">{repo.name}</span>
                                <span class="truncate font-mono text-[13px] text-dim">{repo.path}</span>
                            </div>
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
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
                    <div class="flex flex-col gap-px rounded-md border border-line p-2 max-h-[220px] overflow-y-auto">
                        {repo_rows}
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
                        class="btn-ghost"
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
