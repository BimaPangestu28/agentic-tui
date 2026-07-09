//! The "new run" form (route `/run/new`): collects the goal and options for
//! the selected workspace, optionally runs the goal-refine clarification
//! flow against the server, then starts the pipeline run and navigates to
//! its dashboard.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;
use shared::{StartRunRequest, WorkspaceDto};

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
    let base_input = RwSignal::new(String::new());
    let into_input = RwSignal::new(String::new());
    let verify_input = RwSignal::new(String::new());
    let refine_enabled = RwSignal::new(true);

    let flow = RwSignal::new(FlowState::Editing);

    // Load the workspace list once and pick out the one named by the
    // `workspace` query param, prefilling base/into from its defaults. This
    // app is CSR-only, so component creation and "on mount" are the same
    // point in time.
    {
        let name = workspace_name.clone();
        spawn_local(async move {
            match api::list_workspaces().await {
                Ok(list) => match list.into_iter().find(|w| w.name == name) {
                    Some(found) => {
                        base_input.set(found.base.clone().unwrap_or_default());
                        into_input.set(found.integration.clone().unwrap_or_default());
                        workspace.set(Some(found));
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
            let repo = selected.path.clone();
            spawn_local(async move {
                match api::refine_questions(&repo, &goal).await {
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
            let base = normalize(base_input.get_untracked());
            let into = normalize(into_input.get_untracked());
            let verify = normalize(verify_input.get_untracked());
            flow.set(FlowState::Submitting);
            let navigate = navigate_for_start.clone();
            let request = StartRunRequest {
                workspace: selected,
                goal,
                base,
                into,
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
        flow.set(FlowState::Submitting);
        spawn_local(async move {
            match api::refine_finalize(&selected.path, &original_goal, qa_pairs).await {
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
        let base = normalize(base_input.get_untracked());
        let into = normalize(into_input.get_untracked());
        let verify = normalize(verify_input.get_untracked());
        flow.set(FlowState::Submitting);
        let navigate = navigate_for_plan.clone();
        let request = StartRunRequest {
            workspace: selected,
            goal,
            base,
            into,
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
        view! {
            <div class="new-run-form">
                {error.map(|msg| view! { <p class="error">{msg}</p> })}
                <label class="field">
                    "Goal"
                    <textarea
                        rows="6"
                        prop:value=move || goal_input.get()
                        on:input=move |ev| goal_input.set(event_target_value(&ev))
                    ></textarea>
                </label>
                <label class="field">
                    "Base branch"
                    <input
                        type="text"
                        prop:value=move || base_input.get()
                        on:input=move |ev| base_input.set(event_target_value(&ev))
                    />
                </label>
                <label class="field">
                    "Integration branch"
                    <input
                        type="text"
                        prop:value=move || into_input.get()
                        on:input=move |ev| into_input.set(event_target_value(&ev))
                    />
                </label>
                <label class="field">
                    "Verify command"
                    <input
                        type="text"
                        prop:value=move || verify_input.get()
                        on:input=move |ev| verify_input.set(event_target_value(&ev))
                    />
                </label>
                <label class="field checkbox">
                    <input
                        type="checkbox"
                        prop:checked=move || refine_enabled.get()
                        on:change=move |_| refine_enabled.update(|value| *value = !*value)
                    />
                    "Refine the goal before planning"
                </label>
                <button type="button" on:click=on_start.clone()>
                    {move || {
                        if refine_enabled.get() {
                            "Refine & plan"
                        } else {
                            "Plan"
                        }
                    }}
                </button>
            </div>
        }
    };

    view! {
        <div class="new-run-view">
            <h1>"New run"</h1>
            {move || {
                if let Some(err) = load_error.get() {
                    view! { <p class="error">{err}</p> }.into_any()
                } else if workspace.get().is_none() {
                    view! { <p>"Loading workspace..."</p> }.into_any()
                } else {
                    match flow.get() {
                        FlowState::Editing => render_form(None).into_any(),
                        FlowState::Error(msg) => render_form(Some(msg)).into_any(),
                        FlowState::Submitting => view! { <p>"Working..."</p> }.into_any(),
                        FlowState::Answering {
                            questions,
                            answers,
                            refined_goal,
                            cost,
                        } => {
                            let rows: Vec<_> = questions
                                .iter()
                                .enumerate()
                                .map(|(i, question)| {
                                    let question = question.clone();
                                    let current_answer =
                                        answers.get(i).cloned().unwrap_or_default();
                                    view! {
                                        <div class="refine-question">
                                            <label>
                                                {question}
                                                <input
                                                    type="text"
                                                    prop:value=current_answer
                                                    on:input=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        flow.update(|f| {
                                                            if let FlowState::Answering {
                                                                answers,
                                                                ..
                                                            } = f
                                                            {
                                                                if let Some(slot) =
                                                                    answers.get_mut(i)
                                                                {
                                                                    *slot = value;
                                                                }
                                                            }
                                                        });
                                                    }
                                                />
                                            </label>
                                        </div>
                                    }
                                })
                                .collect();
                            view! {
                                <div class="refine-answering">
                                    <h2>"A few clarifying questions"</h2>
                                    <p class="hint">
                                        "Working goal: " {refined_goal}
                                    </p>
                                    <p class="hint">
                                        {format!("Refine cost so far: ${cost:.4}")}
                                    </p>
                                    {rows}
                                    <button type="button" on:click=on_continue>
                                        "Continue"
                                    </button>
                                </div>
                            }
                            .into_any()
                        }
                        FlowState::Confirming { goal, cost } => {
                            view! {
                                <div class="refine-confirm">
                                    <h2>"Confirm the goal"</h2>
                                    <p class="hint">
                                        {format!("Refine cost so far: ${cost:.4}")}
                                    </p>
                                    <textarea
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
                                    <button type="button" on:click=on_plan.clone()>
                                        "Plan"
                                    </button>
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
