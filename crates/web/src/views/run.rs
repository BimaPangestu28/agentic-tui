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
    // Base pill shared by every report status; the trailing color trio is the
    // only part that varies by status.
    match status {
        EpicStatus::Merged => {
            "inline-flex items-center gap-1.5 justify-self-start rounded-full \
             border px-[9px] py-[3px] text-[12px] font-semibold whitespace-nowrap \
             text-done bg-done/12 border-done/30"
        }
        EpicStatus::Skipped => {
            "inline-flex items-center gap-1.5 justify-self-start rounded-full \
             border px-[9px] py-[3px] text-[12px] font-semibold whitespace-nowrap \
             text-dim bg-todo/10 border-todo/30"
        }
        _ => {
            "inline-flex items-center gap-1.5 justify-self-start rounded-full \
             border px-[9px] py-[3px] text-[12px] font-semibold whitespace-nowrap \
             text-block bg-block/10 border-block/30"
        }
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
/// not all merged yet carries an "on hold" hint listing what it waits on. A
/// blocked epic shows why it is stuck; when the run has finished and the epic
/// has work of its own to redo (Failed or Conflict), it also offers a Retry
/// button that re-runs just that epic.
fn epic_card(
    epic: EpicView,
    hold_label: Option<String>,
    run_id: String,
    retry_enabled: bool,
) -> impl IntoView {
    let can_retry =
        retry_enabled && matches!(epic.status, EpicStatus::Failed | EpicStatus::Conflict);
    let reason = epic.reason.clone();
    let epic_id = epic.id.clone();

    // A card is styled by the column it sits in, which is fixed by its status.
    // `left_accent` tints the 3px left rail, `card_glow` adds the running-card
    // ring, and `status_pill` colors the status chip to match the column.
    let (left_accent, card_glow, status_pill) = match kanban_column(epic.status) {
        KanbanColumn::Todo => (
            "border-l-todo/30",
            "",
            "text-todo bg-todo/10 border-todo/30",
        ),
        KanbanColumn::InProgress => (
            "border-l-prog",
            "ring-1 ring-prog/40",
            "text-prog bg-prog/12 border-prog/40",
        ),
        KanbanColumn::Review => (
            "border-l-review",
            "",
            "text-review bg-review/12 border-review/30",
        ),
        KanbanColumn::Done => ("border-l-done", "", "text-done bg-done/12 border-done/30"),
        KanbanColumn::Blocked => (
            "border-l-block",
            "",
            "text-block bg-block/10 border-block/30",
        ),
    };
    let card_class = format!(
        "flex flex-col gap-2 rounded-md border border-line border-l-[3px] {left_accent} \
         bg-inset py-3 pr-3 pl-4 shadow-card transition hover:-translate-y-px hover:bg-raised \
         {card_glow}"
    );
    let status_class = format!(
        "inline-flex items-center gap-1.5 shrink-0 ml-auto rounded-full border px-2 py-[3px] \
         text-[12px] font-semibold whitespace-nowrap {status_pill}"
    );

    let retrying = RwSignal::new(false);
    let retry_error = RwSignal::new(None::<String>);
    let on_retry = move |_| {
        if retrying.get_untracked() {
            return;
        }
        retrying.set(true);
        retry_error.set(None);
        let run_id = run_id.clone();
        let epic_id = epic_id.clone();
        spawn_local(async move {
            if let Err(err) = api::retry_epic(&run_id, &epic_id).await {
                retry_error.set(Some(err));
            }
            retrying.set(false);
        });
    };

    view! {
        <div class=card_class>
            <div class="text-[13px] font-semibold text-ink leading-[1.35]">{epic.title.clone()}</div>
            {hold_label
                .map(|label| {
                    view! {
                        <span class="inline-flex items-center gap-1.5 self-start rounded-full \
                        border border-dashed border-line-strong bg-surface px-2 py-0.5 \
                        text-[12px] font-semibold text-dim">
                            {label}
                        </span>
                    }
                })}
            {reason
                .map(|text| {
                    view! {
                        <div class="rounded-[5px] border border-block/30 bg-block/10 px-2 py-[5px] \
                        text-[12px] leading-[1.4] text-block">
                            {text}
                        </div>
                    }
                })}
            <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono text-[12px] text-dim whitespace-nowrap">
                    {epic.id.clone()}
                </span>
                {(!epic.repo.is_empty())
                    .then(|| {
                        view! {
                            <span class="font-mono text-[12px] text-dim whitespace-nowrap">
                                {epic.repo.clone()}
                            </span>
                        }
                    })}
                <span class=status_class>{status_label(epic.status)}</span>
            </div>
            {can_retry
                .then(|| {
                    view! {
                        <div class="flex justify-end">
                            <button
                                type="button"
                                class="rounded-[5px] border border-line-strong bg-surface \
                                px-[14px] py-1.5 text-[12px] font-semibold text-ink \
                                transition-colors hover:bg-block/10 hover:border-block/30 \
                                hover:text-block disabled:opacity-60 disabled:cursor-default"
                                disabled=move || retrying.get()
                                on:click=on_retry
                            >
                                {move || if retrying.get() { "Retrying..." } else { "Retry" }}
                            </button>
                        </div>
                    }
                })}
            {move || {
                retry_error
                    .get()
                    .map(|err| {
                        view! {
                            <p class="text-[12px] leading-[1.4] text-danger">{err}</p>
                        }
                    })
            }}
        </div>
    }
}

/// One kanban column: a header with a card count, and its cards.
fn kanban_column_view(
    column: KanbanColumn,
    label: &'static str,
    epics: Vec<EpicView>,
    status_by_id: HashMap<String, EpicStatus>,
    run_id: String,
    retry_enabled: bool,
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
            epic_card(epic, hold_label, run_id.clone(), retry_enabled)
        })
        .collect();
    // The column header text is tinted to match the column's status color.
    let header_color = match column {
        KanbanColumn::Todo => "text-todo",
        KanbanColumn::InProgress => "text-prog",
        KanbanColumn::Review => "text-review",
        KanbanColumn::Done => "text-done",
        KanbanColumn::Blocked => "text-block",
    };
    let header_class = format!(
        "flex items-center gap-2 sticky top-0 px-4 py-3 text-[13px] font-semibold \
         border-b border-line {header_color}"
    );
    view! {
        <div class="flex flex-1 flex-col min-w-[224px] max-h-[640px] snap-start rounded-lg border border-line bg-surface">
            <h3 class=header_class>
                {label} " "
                <span class="ml-auto font-mono text-[12px] text-dim bg-inset rounded-full px-2 py-px">
                    {count}
                </span>
            </h3>
            <div class="flex flex-col gap-3 p-3 overflow-y-auto">{cards}</div>
        </div>
    }
}

/// One log line, tinted by a leading `[tag]` convention when present.
fn log_line_view(line: String) -> AnyView {
    // Every line shares the same frame; only the body text color (for errors)
    // and the leading `[tag]` color change by line type.
    let line_base = "px-6 py-px whitespace-pre-wrap break-words border-l-2 border-transparent \
                     transition-colors hover:bg-surface hover:border-line-strong";
    let lower = line.to_lowercase();
    let (line_class, tag_class) =
        if lower.contains("merged") || lower.contains("passed") || lower.contains("ok (") {
            (format!("{line_base} text-muted"), "text-done")
        } else if lower.contains("conflict") || lower.contains("error") || lower.contains("fail") {
            (format!("{line_base} text-danger"), "text-danger")
        } else if lower.starts_with("[plan]") {
            (format!("{line_base} text-muted"), "text-review")
        } else {
            (format!("{line_base} text-muted"), "text-accent")
        };
    let parsed = line.strip_prefix('[').and_then(|rest| {
        rest.find(']')
            .map(|idx| (format!("[{}]", &rest[..idx]), rest[idx + 1..].to_string()))
    });
    match parsed {
        Some((tag, body)) => view! {
            <div class=line_class>
                <span class=tag_class>{tag}</span>
                {body}
            </div>
        }
        .into_any(),
        None => view! { <div class=line_class>{line}</div> }.into_any(),
    }
}

/// One report row: an epic's id, title, status pill, and cost.
fn report_row(epic: &EpicView) -> impl IntoView {
    view! {
        <div class="grid grid-cols-[84px_1fr_auto_auto] items-center gap-4 py-3 border-b border-line last:border-b-0">
            <span class="font-mono text-[12px] text-dim">{epic.id.clone()}</span>
            <span class="text-[13px] text-ink min-w-0">{epic.title.clone()}</span>
            <span class=report_status_class(epic.status)>{status_label(epic.status)}</span>
            <span class="font-mono text-[13px] text-muted justify-self-end">
                {format!("${:.2}", epic.cost)}
            </span>
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
                <div class="flex flex-col gap-2">
                    <h4 class="font-mono text-[13px] font-semibold text-muted">{heading}</h4>
                    <div class="flex flex-col">{rows}</div>
                    {group_merged
                        .then(|| {
                            view! {
                                <div class="flex items-center gap-2 rounded-md border border-done/30 \
                                bg-done/12 px-4 py-3 text-[13px] text-muted">
                                    "Merged epics are on this repo's integration branch."
                                </div>
                            }
                        })}
                </div>
            }
        })
        .collect();

    view! {
        <div class="flex flex-col gap-4 rounded-lg border border-line bg-surface p-6 shadow-card">
            <h3 class="flex items-center gap-2 text-[18px] font-semibold">"Run finished"</h3>
            {app
                .error
                .clone()
                .map(|err| {
                    view! {
                        <p class="flex items-start gap-2 rounded-md border border-block/30 \
                        bg-block/10 px-4 py-3 text-[13px] text-danger before:content-['⚠']">
                            {err}
                        </p>
                    }
                })}
            {groups}
            <div class="flex items-baseline justify-between pt-3 border-t border-line-strong text-[15px]">
                <span>"Total cost"</span>
                <span class="font-mono font-bold text-ink text-[18px]">
                    {format!("${:.4}", app.total_cost)}
                </span>
            </div>
            {any_merged
                .then(|| {
                    view! {
                        <div class="flex items-center gap-2 rounded-md border border-done/30 \
                        bg-done/12 px-4 py-3 text-[13px] text-muted">
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

    let resuming = RwSignal::new(false);
    let resume_error = RwSignal::new(None::<String>);
    let on_resume = {
        let run_id = run_id.clone();
        move |_| {
            if resuming.get_untracked() {
                return;
            }
            resuming.set(true);
            resume_error.set(None);
            let run_id = run_id.clone();
            spawn_local(async move {
                if let Err(err) = api::resume_run(&run_id).await {
                    resume_error.set(Some(err));
                }
                resuming.set(false);
            });
        }
    };

    view! {
        <div class="flex flex-col gap-6">
            {move || {
                abort_error
                    .get()
                    .map(|err| {
                        view! {
                            <p class="flex items-start gap-2 rounded-md border border-block/30 \
                            bg-block/10 px-4 py-3 text-[13px] text-danger">
                                {err}
                            </p>
                        }
                    })
            }}
            {move || {
                resume_error
                    .get()
                    .map(|err| {
                        view! {
                            <p class="flex items-start gap-2 rounded-md border border-block/30 \
                            bg-block/10 px-4 py-3 text-[13px] text-danger">
                                {err}
                            </p>
                        }
                    })
            }}
            {move || match app.get() {
                None => {
                    view! {
                        <div class="flex items-center gap-2 rounded-md border border-line \
                        bg-surface px-4 py-3 text-[13px] text-muted">
                            <span class="size-[13px] animate-spin rounded-full border-2 \
                            border-line-strong border-t-accent"></span>
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
                    let is_finished = matches!(snapshot.phase, Phase::Done | Phase::Failed);
                    // Resumable when the run failed with epics still unfinished:
                    // the restart-recovery path and ordinary failures both land
                    // here, and resume re-runs every non-merged epic.
                    let can_resume = matches!(snapshot.phase, Phase::Failed)
                        && snapshot
                            .epics
                            .iter()
                            .any(|epic| !matches!(epic.status, EpicStatus::Merged));
                    let restart_error = snapshot.error.clone();
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
                                run_id.clone(),
                                is_finished,
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
                        {restart_error
                            .clone()
                            .filter(|_| can_resume)
                            .map(|reason| {
                                view! {
                                    <div class="flex items-center gap-2 rounded-md border \
                                    border-block/30 bg-block/10 px-4 py-3 text-[13px] text-danger">
                                        {reason}
                                    </div>
                                }
                            })}
                        <div class="grid grid-cols-[1fr_auto] items-start gap-x-6 gap-y-4 \
                        rounded-lg border border-line bg-surface p-6 shadow-card">
                            <div>
                                <div class="max-w-[68ch] whitespace-pre-wrap text-[18px] \
                                font-semibold leading-[1.4] tracking-tight text-ink">
                                    {snapshot.goal.clone()}
                                </div>
                                <div class="mt-2 inline-flex items-center gap-1.5 font-mono \
                                text-[13px] text-muted">
                                    {snapshot.workspace.clone()}
                                    {(repo_count > 0)
                                        .then(|| format!(" \u{00b7} {repo_count} repos"))}
                                </div>
                            </div>
                            {(!is_finished)
                                .then(|| {
                                    view! {
                                        <div class="col-start-2 row-start-1 flex items-center gap-2">
                                            <button
                                                type="button"
                                                class="inline-flex items-center justify-center gap-2 \
                                                rounded-md border border-block/30 bg-transparent \
                                                px-[18px] py-2.5 min-h-[38px] text-[14px] font-medium \
                                                text-danger transition-colors hover:bg-block/10 \
                                                hover:border-danger disabled:opacity-50 \
                                                disabled:cursor-not-allowed"
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
                            {can_resume
                                .then(|| {
                                    view! {
                                        <div class="col-start-2 row-start-1 flex items-center gap-2">
                                            <button
                                                type="button"
                                                class="inline-flex items-center justify-center gap-2 \
                                                rounded-md bg-accent px-[18px] py-2.5 min-h-[38px] \
                                                text-[14px] font-semibold text-accent-fg shadow-card \
                                                transition-colors hover:bg-accent-hover \
                                                active:bg-accent-press disabled:opacity-50 \
                                                disabled:cursor-not-allowed"
                                                disabled=move || resuming.get()
                                                on:click=on_resume.clone()
                                            >
                                                {move || {
                                                    if resuming.get() { "Resuming..." } else { "Resume run" }
                                                }}
                                            </button>
                                        </div>
                                    }
                                })}
                            <div class="col-span-2 flex items-baseline gap-1.5 font-mono \
                            text-[13px] text-muted">
                                <span class="text-ink font-semibold">
                                    {format!("${:.4}", snapshot.total_cost)}
                                </span>
                                <span class="text-dim">" spent"</span>
                            </div>
                        </div>
                        <div class="flex gap-4 overflow-x-auto pb-3 snap-x snap-proximity">
                            {columns}
                        </div>
                        <div class="max-h-[300px] overflow-y-auto rounded-lg border border-line \
                        bg-page py-3 font-mono text-[13px] leading-[1.7] scroll-smooth">
                            {log_lines}
                        </div>
                        {is_finished.then(|| final_report(&snapshot))}
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
