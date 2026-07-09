//! The live run dashboard (route `/run/:id`): opens a WebSocket to the
//! run's event stream and renders the latest `App` snapshot as a header with
//! goal/workspace/budget, a five-column kanban board, a scrolling log pane,
//! an abort button, and, once the run finishes, a final report.

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

/// A short human label for an epic status. `EpicStatus` has no `Display`
/// impl (it is a wire type shared with the server), so the card and the
/// final report render this instead.
fn status_label(status: EpicStatus) -> &'static str {
    match status {
        EpicStatus::Pending => "pending",
        EpicStatus::Running => "running",
        EpicStatus::Verifying => "verifying",
        EpicStatus::Merged => "merged",
        EpicStatus::Failed => "failed",
        EpicStatus::Skipped => "skipped",
        EpicStatus::Conflict => "conflict",
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

/// One kanban card. In the Todo column, a pending epic whose dependencies
/// have not all merged yet gets an "on hold" hint, matching the old TUI's
/// bucketing via `shared::is_on_hold`.
fn epic_card(epic: EpicView, on_hold: bool) -> impl IntoView {
    view! {
        <li class="kanban-card">
            <div class="kanban-card-title">{epic.title.clone()}</div>
            <div class="kanban-card-id">{epic.id.clone()}</div>
            <div class="kanban-card-status">{status_label(epic.status)}</div>
            {on_hold.then(|| view! { <div class="kanban-card-hold">"on hold"</div> })}
        </li>
    }
}

/// One kanban column: a header and its cards.
fn kanban_column_view(
    column: KanbanColumn,
    label: &'static str,
    epics: Vec<EpicView>,
    status_by_id: HashMap<String, EpicStatus>,
) -> impl IntoView {
    let cards: Vec<_> = epics
        .into_iter()
        .map(|epic| {
            let on_hold =
                column == KanbanColumn::Todo && is_on_hold(&epic.depends_on, &status_by_id);
            epic_card(epic, on_hold)
        })
        .collect();
    view! {
        <div class="kanban-column">
            <h3>{label}</h3>
            <ul class="kanban-cards">{cards}</ul>
        </div>
    }
}

/// The final report shown once the run reaches `Phase::Done` or
/// `Phase::Failed`: per-epic final status, total cost, and a reminder about
/// the integration branch for any epic that merged. `App` does not carry
/// the integration branch name over the wire, so the reminder stays
/// generic; the workspace name is still shown for context.
fn final_report(app: &App) -> impl IntoView {
    let any_merged = app
        .epics
        .iter()
        .any(|epic| epic.status == EpicStatus::Merged);
    let rows: Vec<_> = app
        .epics
        .iter()
        .map(|epic| {
            view! {
                <li class="report-row">
                    <span class="report-id">{epic.id.clone()}</span>
                    <span class="report-title">{epic.title.clone()}</span>
                    <span class="report-status">{status_label(epic.status)}</span>
                    <span class="report-cost">{format!("${:.4}", epic.cost)}</span>
                </li>
            }
        })
        .collect();

    view! {
        <div class="final-report">
            <h2>"Final report"</h2>
            {app.error.clone().map(|err| view! { <p class="error">{err}</p> })}
            <ul class="report-rows">{rows}</ul>
            <p class="report-total">{format!("Total cost: ${:.4}", app.total_cost)}</p>
            {any_merged
                .then(|| {
                    view! {
                        <p class="report-hint">
                            "Merged epics were integrated onto this workspace's integration branch. Check it before deleting any run branches."
                        </p>
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

    // Open the WebSocket once and hold the handle in arena storage for the
    // component's lifetime; dropping it early would close the connection.
    // This app is CSR-only, so component creation and "on mount" are the
    // same point in time.
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
            <h1>"Run"</h1>
            {move || {
                abort_error.get().map(|err| view! { <p class="error">{err}</p> })
            }}
            {move || match app.get() {
                None => view! { <p>"Connecting..."</p> }.into_any(),
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
                        .map(|line| view! { <div class="log-line">{line}</div> })
                        .collect();

                    view! {
                        <div class="run-header">
                            <p class="run-goal">{snapshot.goal.clone()}</p>
                            <p class="run-workspace">{snapshot.workspace.clone()}</p>
                            <div class="budget-bar">
                                <div class="budget-bar-fill" style=format!("width: {budget_pct}%")></div>
                            </div>
                            <p class="budget-text">
                                {format!("${:.4} / ${:.4}", snapshot.total_cost, snapshot.budget)}
                            </p>
                            {(!is_finished)
                                .then(|| {
                                    view! {
                                        <button
                                            type="button"
                                            disabled=move || aborting.get()
                                            on:click=on_abort.clone()
                                        >
                                            {move || if aborting.get() { "Aborting..." } else { "Abort" }}
                                        </button>
                                    }
                                })}
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
