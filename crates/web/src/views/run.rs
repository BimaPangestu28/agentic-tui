//! The live run dashboard (route `/run/:id`): opens a WebSocket to the
//! run's event stream and renders the latest `App` snapshot as a header with
//! goal/workspace/budget, a five-column kanban board, a scrolling log pane,
//! an abort button, and, once the run finishes, a final report. The markup
//! mirrors the `run.html` design mockup so the design system lays it out as
//! intended.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use shared::{is_on_hold, kanban_column, App, EpicStatus, EpicView, KanbanColumn, Phase};
use web_sys::WebSocket;

use crate::api;
use crate::ws;

/// Kanban columns in the fixed display order, paired with their header text.
const COLUMNS: [(KanbanColumn, &str); 5] = [
    (KanbanColumn::Todo, "Todo"),
    (KanbanColumn::InProgress, "In progress"),
    (KanbanColumn::Review, "Review"),
    (KanbanColumn::Done, "Done"),
    (KanbanColumn::Blocked, "Blocked"),
];

/// A short human label for an epic status. `EpicStatus` has no `Display` impl
/// (it is a wire type shared with the server), so the card and the report
/// render this instead.
fn status_label(status: EpicStatus) -> &'static str {
    match status {
        EpicStatus::Pending => "Pending",
        EpicStatus::Running => "Running",
        EpicStatus::Verifying => "Verifying",
        EpicStatus::Merged => "Merged",
        EpicStatus::Failed => "Failed",
        EpicStatus::Skipped => "Skipped",
        EpicStatus::Conflict => "Conflict",
    }
}

/// The report-row status pill class: green for merged, neutral for skipped,
/// red for anything blocked (failed / conflict).
fn report_status_class(status: EpicStatus) -> &'static str {
    match status {
        EpicStatus::Merged => "report-status done",
        EpicStatus::Skipped => "report-status skipped",
        _ => "report-status blocked",
    }
}

/// The epics belonging to one kanban column, in their original order.
fn column_epics(epics: &[EpicView], column: KanbanColumn) -> Vec<EpicView> {
    epics
        .iter()
        .filter(|epic| kanban_column(epic.status) == column)
        .cloned()
        .collect()
}

/// One kanban card. In the Todo column, a pending epic whose dependencies have
/// not all merged yet carries an "on hold" hint listing what it waits on.
fn epic_card(epic: EpicView, hold_label: Option<String>) -> impl IntoView {
    view! {
        <div class="kanban-card">
            <div class="kanban-card-title">{epic.title.clone()}</div>
            {hold_label.map(|label| view! { <span class="kanban-card-hold">{label}</span> })}
            <div class="kanban-card-meta">
                <span class="kanban-card-id">{epic.id.clone()}</span>
                {(!epic.repo.is_empty())
                    .then(|| view! { <span class="kanban-card-repo">{epic.repo.clone()}</span> })}
                <span class="kanban-card-status">{status_label(epic.status)}</span>
            </div>
        </div>
    }
}

/// One kanban column: a header with a card count, and its cards.
fn kanban_column_view(
    column: KanbanColumn,
    label: &'static str,
    epics: Vec<EpicView>,
    status_by_id: HashMap<String, EpicStatus>,
) -> impl IntoView {
    let count = epics.len();
    let cards: Vec<_> = epics
        .into_iter()
        .map(|epic| {
            let hold_label =
                if column == KanbanColumn::Todo && is_on_hold(&epic.depends_on, &status_by_id) {
                    let waiting: Vec<String> = epic
                        .depends_on
                        .iter()
                        .filter(|dep| status_by_id.get(*dep) != Some(&EpicStatus::Merged))
                        .cloned()
                        .collect();
                    Some(if waiting.is_empty() {
                        "on hold".to_string()
                    } else {
                        format!("on hold \u{00b7} waits on {}", waiting.join(", "))
                    })
                } else {
                    None
                };
            epic_card(epic, hold_label)
        })
        .collect();
    view! {
        <div class="kanban-column">
            <h3>{label} " " <span class="count">{count}</span></h3>
            <div class="kanban-cards">{cards}</div>
        </div>
    }
}

/// One log line, tinted by a leading `[tag]` convention when present.
fn log_line_view(line: String) -> AnyView {
    let lower = line.to_lowercase();
    let class = if lower.contains("merged") || lower.contains("passed") || lower.contains("ok (") {
        "log-line done"
    } else if lower.contains("conflict") || lower.contains("error") || lower.contains("fail") {
        "log-line err"
    } else if lower.starts_with("[plan]") {
        "log-line plan"
    } else {
        "log-line"
    };
    let parsed = line.strip_prefix('[').and_then(|rest| {
        rest.find(']')
            .map(|idx| (format!("[{}]", &rest[..idx]), rest[idx + 1..].to_string()))
    });
    match parsed {
        Some((tag, body)) => view! {
            <div class=class>
                <span class="tag">{tag}</span>
                {body}
            </div>
        }
        .into_any(),
        None => view! { <div class=class>{line}</div> }.into_any(),
    }
}

/// One report row: an epic's id, title, status pill, and cost.
fn report_row(epic: &EpicView) -> impl IntoView {
    view! {
        <div class="report-row">
            <span class="report-id">{epic.id.clone()}</span>
            <span class="report-title">{epic.title.clone()}</span>
            <span class=report_status_class(epic.status)>{status_label(epic.status)}</span>
            <span class="report-cost">{format!("${:.2}", epic.cost)}</span>
        </div>
    }
}

/// Groups epics by repo, preserving each repo's first-seen order. Epics with
/// no repo tag are collected under one group and moved to the end, since an
/// empty repo carries no ordering signal of its own.
fn group_epics_by_repo(epics: &[EpicView]) -> Vec<(String, Vec<EpicView>)> {
    let mut groups: Vec<(String, Vec<EpicView>)> = Vec::new();
    for epic in epics {
        match groups.iter_mut().find(|(repo, _)| repo == &epic.repo) {
            Some((_, group_epics)) => group_epics.push(epic.clone()),
            None => groups.push((epic.repo.clone(), vec![epic.clone()])),
        }
    }
    if let Some(idx) = groups.iter().position(|(repo, _)| repo.is_empty()) {
        let unassigned = groups.remove(idx);
        groups.push(unassigned);
    }
    groups
}

/// The final report shown once the run reaches `Phase::Done` or `Phase::Failed`.
/// Rows are grouped by repo so a multi-repo run reads as one section per repo,
/// each followed by a note that the repo's integration branch holds its
/// merged work.
fn final_report(app: &App) -> impl IntoView {
    let any_merged = app
        .epics
        .iter()
        .any(|epic| epic.status == EpicStatus::Merged);

    let groups: Vec<_> = group_epics_by_repo(&app.epics)
        .into_iter()
        .map(|(repo, epics)| {
            let heading = if repo.is_empty() {
                "No repo assigned".to_string()
            } else {
                repo
            };
            let group_merged = epics.iter().any(|epic| epic.status == EpicStatus::Merged);
            let rows: Vec<_> = epics.iter().map(report_row).collect();
            view! {
                <div class="report-group">
                    <h4 class="report-group-heading">{heading}</h4>
                    <div class="report-rows">{rows}</div>
                    {group_merged
                        .then(|| {
                            view! {
                                <div class="report-hint">
                                    "Merged epics are on this repo's integration branch."
                                </div>
                            }
                        })}
                </div>
            }
        })
        .collect();

    view! {
        <div class="final-report">
            <h3>"Run finished"</h3>
            {app.error.clone().map(|err| view! { <p class="error">{err}</p> })}
            {groups}
            <div class="report-total">
                <span>"Total cost"</span>
                <span class="amount">{format!("${:.4}", app.total_cost)}</span>
            </div>
            {any_merged
                .then(|| {
                    view! {
                        <div class="report-hint">
                            "Merged work is on this workspace's integration branch. Review it and open a PR when ready."
                        </div>
                    }
                })}
        </div>
    }
}

#[component]
pub fn Run() -> impl IntoView {
    let params = use_params_map();
    let run_id = params.get_untracked().get("id").unwrap_or_default();

    let app = RwSignal::new(None::<App>);
    let aborting = RwSignal::new(false);
    let abort_error = RwSignal::new(None::<String>);

    // Open the WebSocket once and hold the handle for the component's lifetime;
    // dropping it early would close the connection. This app is CSR-only, so
    // component creation and "on mount" are the same point in time.
    let _socket: StoredValue<Option<WebSocket>, LocalStorage> =
        StoredValue::new_local(ws::connect(&run_id, app));

    let on_abort = {
        let run_id = run_id.clone();
        move |_| {
            if aborting.get_untracked() {
                return;
            }
            aborting.set(true);
            abort_error.set(None);
            let run_id = run_id.clone();
            spawn_local(async move {
                if let Err(err) = api::abort_run(&run_id).await {
                    abort_error.set(Some(err));
                }
                aborting.set(false);
            });
        }
    };

    view! {
        <div class="run-view">
            {move || {
                abort_error.get().map(|err| view! { <p class="error">{err}</p> })
            }}
            {move || match app.get() {
                None => {
                    view! {
                        <div class="run-status-banner">
                            <span class="spinner"></span>
                            "Connecting to the run..."
                        </div>
                    }
                        .into_any()
                }
                Some(snapshot) => {
                    let status_by_id: HashMap<String, EpicStatus> = snapshot
                        .epics
                        .iter()
                        .map(|epic| (epic.id.clone(), epic.status))
                        .collect();
                    let budget_pct = if snapshot.budget > 0.0 {
                        (snapshot.total_cost / snapshot.budget * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    };
                    let is_finished = matches!(snapshot.phase, Phase::Done | Phase::Failed);
                    // The run snapshot has no repo list of its own; derive the
                    // repo count from the distinct non-empty repos tagged on
                    // its epics rather than fabricate one.
                    let repo_count = {
                        let mut seen: Vec<&str> = Vec::new();
                        for epic in &snapshot.epics {
                            if !epic.repo.is_empty() && !seen.contains(&epic.repo.as_str()) {
                                seen.push(&epic.repo);
                            }
                        }
                        seen.len()
                    };
                    let columns: Vec<_> = COLUMNS
                        .into_iter()
                        .map(|(column, label)| {
                            kanban_column_view(
                                column,
                                label,
                                column_epics(&snapshot.epics, column),
                                status_by_id.clone(),
                            )
                        })
                        .collect();
                    let log_lines: Vec<_> = snapshot
                        .log
                        .iter()
                        .cloned()
                        .map(log_line_view)
                        .collect();

                    view! {
                        <div class="run-header">
                            <div>
                                <div class="run-goal">{snapshot.goal.clone()}</div>
                                <div class="run-workspace">
                                    {snapshot.workspace.clone()}
                                    {(repo_count > 0)
                                        .then(|| format!(" \u{00b7} {repo_count} repos"))}
                                </div>
                            </div>
                            {(!is_finished)
                                .then(|| {
                                    view! {
                                        <div class="run-actions">
                                            <button
                                                type="button"
                                                class="btn-danger"
                                                disabled=move || aborting.get()
                                                on:click=on_abort.clone()
                                            >
                                                {move || {
                                                    if aborting.get() { "Aborting..." } else { "Abort run" }
                                                }}
                                            </button>
                                        </div>
                                    }
                                })}
                            <div class="budget">
                                <div class="budget-text">
                                    <span>
                                        <span class="spent">
                                            {format!("${:.4}", snapshot.total_cost)}
                                        </span>
                                        {format!(" / ${:.4}", snapshot.budget)}
                                    </span>
                                    <span>{format!("{budget_pct:.1}% of budget")}</span>
                                </div>
                                <div class="budget-bar">
                                    <div
                                        class="budget-bar-fill"
                                        style=format!("width: {budget_pct}%")
                                    ></div>
                                </div>
                            </div>
                        </div>
                        <div class="kanban-board">{columns}</div>
                        <div class="log-pane">{log_lines}</div>
                        {is_finished.then(|| final_report(&snapshot))}
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
