//! The multi-run dashboard (route `/`): every run started this session,
//! live and finished, with an aggregate overview, a global kanban board
//! across all runs, and runs grouped by workspace. The markup mirrors the
//! `runs.html` design mockup so the design system lays it out as intended.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use shared::{kanban_column, EpicStatus, EpicView, KanbanColumn, Phase, RunSummary};

use crate::api;

/// How often the dashboard polls `GET /api/runs` for fresh phases, epics,
/// and costs. Short enough to feel live, long enough to stay cheap.
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// Kanban columns in the fixed display order, paired with their header text.
const COLUMNS: [(KanbanColumn, &str); 5] = [
    (KanbanColumn::Todo, "Todo"),
    (KanbanColumn::InProgress, "In progress"),
    (KanbanColumn::Review, "Review"),
    (KanbanColumn::Done, "Done"),
    (KanbanColumn::Blocked, "Blocked"),
];

/// "1 repo" / "3 repos": a count with a naively pluralized noun (append "s"
/// past one). Keeps the dashboard from printing "1 repos" or "1 runs".
fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// A short human label for an epic status. `EpicStatus` has no `Display`
/// impl (it is a wire type shared with the server), so the card renders
/// this instead. Kept local rather than shared with `views::run` so the two
/// views stay independent.
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

/// Phase -> the color utilities (text, tint background, border) for its
/// run-phase pill. `Phase` has no `Running` variant; `Implementing` is the
/// phase shown to the user as "Running".
fn phase_class(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => "text-review bg-review/12 border-review/30",
        Phase::Implementing => "text-prog bg-prog/12 border-prog/40",
        Phase::Done => "text-done bg-done/12 border-done/30",
        Phase::Failed => "text-block bg-block/10 border-block/30",
    }
}

/// Phase -> the color (and pulse) utilities for a run card's leading phase
/// dot. Active phases (planning, running) pulse; terminal phases are static.
fn phase_dot_class(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => "bg-review animate-pulse-dot",
        Phase::Implementing => "bg-prog animate-pulse-dot",
        Phase::Done => "bg-done",
        Phase::Failed => "bg-block",
    }
}

/// Phase -> the display label shown in the phase badge.
fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => "Planning",
        Phase::Implementing => "Running",
        Phase::Done => "Done",
        Phase::Failed => "Failed",
    }
}

/// Kanban column -> the background-color utility used for the status
/// distribution bar segment and its legend swatch.
fn column_class(column: KanbanColumn) -> &'static str {
    match column {
        KanbanColumn::Todo => "bg-todo",
        KanbanColumn::InProgress => "bg-prog",
        KanbanColumn::Review => "bg-review",
        KanbanColumn::Done => "bg-done",
        KanbanColumn::Blocked => "bg-block",
    }
}

/// Kanban column -> the text-color utility for its board column heading and
/// the leading status dot (which inherits `currentColor`).
fn column_head_class(column: KanbanColumn) -> &'static str {
    match column {
        KanbanColumn::Todo => "text-todo",
        KanbanColumn::InProgress => "text-prog",
        KanbanColumn::Review => "text-review",
        KanbanColumn::Done => "text-done",
        KanbanColumn::Blocked => "text-block",
    }
}

/// Kanban column -> the left-border accent (and, for the active column, a
/// subtle ring) applied to each epic card in that column.
fn column_card_class(column: KanbanColumn) -> &'static str {
    match column {
        KanbanColumn::Todo => "border-l-todo/30",
        KanbanColumn::InProgress => "border-l-prog ring-1 ring-prog/40",
        KanbanColumn::Review => "border-l-review/30",
        KanbanColumn::Done => "border-l-done/30",
        KanbanColumn::Blocked => "border-l-block/30",
    }
}

/// Kanban column -> the color utilities for an epic card's status pill.
fn column_status_class(column: KanbanColumn) -> &'static str {
    match column {
        KanbanColumn::Todo => "text-todo bg-todo/10 border-todo/30",
        KanbanColumn::InProgress => "text-prog bg-prog/12 border-prog/40",
        KanbanColumn::Review => "text-review bg-review/12 border-review/30",
        KanbanColumn::Done => "text-done bg-done/12 border-done/30",
        KanbanColumn::Blocked => "text-block bg-block/10 border-block/30",
    }
}

/// The overview card, global kanban board, and workspace-grouped runs list.
/// Called only when `runs` is non-empty; the caller renders the empty state
/// instead when there is nothing to show.
fn dashboard_body(runs: &[RunSummary]) -> impl IntoView {
    let total_runs = runs.len();
    let active_runs = runs
        .iter()
        .filter(|run| matches!(run.phase, Phase::Planning | Phase::Implementing))
        .count();
    let total_spend: f64 = runs.iter().map(|run| run.total_cost).sum();

    // Every epic across every run, paired with that run's workspace label
    // for the global board's workspace tag.
    let board_epics: Vec<(String, EpicView)> = runs
        .iter()
        .flat_map(|run| {
            run.epics
                .iter()
                .cloned()
                .map(move |epic| (run.workspace.clone(), epic))
        })
        .collect();
    let total_epics = board_epics.len();

    let column_counts: Vec<(KanbanColumn, &'static str, &'static str, usize)> = COLUMNS
        .into_iter()
        .map(|(column, label)| {
            let count = board_epics
                .iter()
                .filter(|(_, epic)| kanban_column(epic.status) == column)
                .count();
            (column, label, column_class(column), count)
        })
        .collect();

    let segs: Vec<_> = column_counts
        .iter()
        .map(|(_, _, class, count)| {
            let pct = if total_epics == 0 {
                0.0
            } else {
                *count as f64 / total_epics as f64 * 100.0
            };
            view! {
                <div
                    class=format!("h-full transition-[width] duration-[600ms] {class}")
                    style=format!("width: {pct}%")
                ></div>
            }
        })
        .collect();

    let legend: Vec<_> = column_counts
        .iter()
        .map(|(_, label, class, count)| {
            view! {
                <span class="flex items-center gap-[7px] text-[12px] text-muted">
                    <span class=format!("size-[9px] flex-none rounded-[3px] {class}")></span>
                    {*label}
                    <span class="font-mono font-semibold text-ink">{*count}</span>
                </span>
            }
        })
        .collect();

    let board_columns: Vec<_> = column_counts
        .iter()
        .map(|(column, label, _, count)| {
            let cards: Vec<_> = board_epics
                .iter()
                .filter(|(_, epic)| kanban_column(epic.status) == *column)
                .map(|(workspace, epic)| {
                    view! {
                        <div class=format!(
                            "flex flex-col gap-2 rounded-md border border-line border-l-[3px] bg-inset py-3 pr-3 pl-4 shadow-card transition hover:-translate-y-px hover:bg-raised {}",
                            column_card_class(*column),
                        )>
                            <div class="inline-flex items-center gap-1.5 pb-0.5 font-mono text-[12px] text-dim before:text-[10px] before:text-accent/75 before:content-['⬡']">
                                {workspace.clone()}
                            </div>
                            <div class="text-[13px] font-semibold leading-[1.35] text-ink">
                                {epic.title.clone()}
                            </div>
                            <div class="flex flex-wrap items-center gap-2">
                                <span class="whitespace-nowrap font-mono text-[12px] text-dim">
                                    {epic.id.clone()}
                                </span>
                                <span class=format!(
                                    "ml-auto inline-flex flex-none items-center gap-1.5 whitespace-nowrap rounded-full border px-2 py-[3px] text-[12px] font-semibold {}",
                                    column_status_class(*column),
                                )>
                                    {status_label(epic.status)}
                                </span>
                            </div>
                        </div>
                    }
                })
                .collect();
            view! {
                <div class="flex min-w-[224px] max-h-[640px] flex-1 flex-col snap-start rounded-lg border border-line bg-surface">
                    <h3 class=format!(
                        "sticky top-0 flex items-center gap-2 border-b border-line px-4 py-3 text-[13px] font-semibold before:size-2 before:flex-none before:rounded-full before:bg-current before:content-[''] {}",
                        column_head_class(*column),
                    )>
                        {*label} " "
                        <span class="ml-auto rounded-full bg-inset px-2 py-px font-mono text-[12px] text-dim">
                            {*count}
                        </span>
                    </h3>
                    <div class="flex flex-col gap-3 overflow-y-auto p-3 empty:after:block empty:after:py-4 empty:after:text-center empty:after:text-dim empty:after:content-['—']">
                        {cards}
                    </div>
                </div>
            }
        })
        .collect();

    // Runs grouped by workspace, in the order each workspace first appears
    // in the run list.
    let mut groups: Vec<(String, String, Vec<RunSummary>)> = Vec::new();
    for run in runs {
        match groups
            .iter_mut()
            .find(|(name, _, _)| *name == run.workspace)
        {
            Some(group) => group.2.push(run.clone()),
            None => {
                // Summarize the group's repos by count. The workspace name is
                // already shown beside this, so repeating a single repo's name
                // (often identical to the workspace) just read as "happy happy".
                let repo_summary = plural(run.repos.len(), "repo");
                groups.push((run.workspace.clone(), repo_summary, vec![run.clone()]));
            }
        }
    }

    let ws_groups: Vec<_> = groups
        .into_iter()
        .map(|(name, repo_summary, group_runs)| {
            let count = group_runs.len();
            let new_run_href = format!("/run/new?workspace={name}");
            let cards: Vec<_> = group_runs
                .into_iter()
                .map(|run| {
                    let phase_color = phase_class(run.phase);
                    let dot = phase_dot_class(run.phase);
                    let href = format!("/run/{}", run.id);
                    view! {
                        <A
                            attr:class="grid grid-cols-[12px_minmax(0,1fr)_auto] items-center gap-6 rounded-lg border border-line bg-surface px-6 py-4 text-inherit no-underline shadow-card transition hover:-translate-y-px hover:border-accent/40 hover:bg-inset"
                            href=href
                        >
                            <span class=format!("size-2.5 rounded-full {dot}")></span>
                            <div class="flex min-w-0 flex-col gap-1">
                                <div class="truncate text-[14px] font-medium leading-[1.4] text-ink">
                                    {run.goal.clone()}
                                </div>
                                <div class="truncate font-mono text-[12px] text-dim">
                                    {format!(
                                        "{} \u{00b7} {}",
                                        plural(run.epics.len(), "epic"),
                                        plural(run.repos.len(), "repo"),
                                    )}
                                </div>
                            </div>
                            <div class="grid grid-cols-[auto_128px] items-center gap-4">
                                <span class=format!(
                                    "inline-flex items-center gap-1.5 justify-self-start whitespace-nowrap rounded-full border px-2 py-[3px] text-[12px] font-semibold {phase_color}",
                                )>
                                    {phase_label(run.phase)}
                                </span>
                                <span class="font-mono text-[12px] text-muted">
                                    {format!("${:.2}", run.total_cost)}
                                </span>
                            </div>
                        </A>
                    }
                })
                .collect();
            view! {
                <div class="flex flex-col gap-3">
                    <div class="flex items-center gap-3 border-b border-line px-1 pb-3">
                        <span class="text-[15px] text-accent">"\u{2b21}"</span>
                        <span class="font-mono text-[15px] font-semibold text-ink">{name}</span>
                        <span class="font-mono text-[12px] text-dim">{repo_summary}</span>
                        <span class="ml-auto"></span>
                        <span class="rounded-full border border-line bg-inset px-[9px] py-px text-[12px] text-dim">
                            {plural(count, "run")}
                        </span>
                        <A
                            attr:class="inline-flex items-center rounded-md px-3 py-1.5 text-[13px] font-medium text-muted no-underline transition-colors hover:bg-inset hover:text-ink"
                            href=new_run_href
                        >
                            "+ New run"
                        </A>
                    </div>
                    <div class="flex flex-col gap-3">{cards}</div>
                </div>
            }
        })
        .collect();

    view! {
        <div class="mb-8">
            <div class="grid grid-cols-[auto_1px_1fr] items-center gap-8 rounded-lg border border-line bg-surface px-8 py-6 shadow-card">
                <div class="flex gap-12">
                    <div class="flex flex-col gap-2">
                        <span class="whitespace-nowrap text-[12px] font-bold uppercase tracking-[0.06em] text-dim">
                            "Active loops"
                        </span>
                        <span class="text-[28px] font-semibold leading-none tracking-[-0.02em] text-ink tabular-nums">
                            {active_runs}
                            " "
                            <span class="text-[18px] font-medium text-dim">
                                {format!("/ {total_runs}")}
                            </span>
                        </span>
                    </div>
                    <div class="flex flex-col gap-2">
                        <span class="whitespace-nowrap text-[12px] font-bold uppercase tracking-[0.06em] text-dim">
                            "Epics"
                        </span>
                        <span class="text-[28px] font-semibold leading-none tracking-[-0.02em] text-ink tabular-nums">
                            {total_epics}
                        </span>
                    </div>
                    <div class="flex flex-col gap-2">
                        <span class="whitespace-nowrap text-[12px] font-bold uppercase tracking-[0.06em] text-dim">
                            "Total spend"
                        </span>
                        <span class="font-mono text-[22px] font-semibold leading-none tracking-[-0.02em] text-accent tabular-nums">
                            {format!("${total_spend:.2}")}
                        </span>
                    </div>
                </div>
                <div class="w-px self-stretch bg-line"></div>
                <div class="flex min-w-0 flex-col gap-3">
                    <div class="text-[12px] font-bold uppercase tracking-[0.06em] text-dim">
                        "Epic status"
                    </div>
                    <div class="flex h-3.5 gap-0.5 overflow-hidden rounded-full bg-raised">{segs}</div>
                    <div class="mt-4 flex flex-wrap gap-4">{legend}</div>
                </div>
            </div>
        </div>

        <div class="mt-8 mb-3 flex items-baseline gap-3">
            <h2 class="text-[18px] font-semibold leading-tight tracking-[-0.01em]">"Board"</h2>
            <span class="text-[13px] text-dim">"Every epic, across every run"</span>
        </div>
        <div class="flex gap-4 overflow-x-auto pb-3 snap-x snap-proximity">{board_columns}</div>

        <div class="mt-8 mb-3 flex items-baseline gap-3">
            <h2 class="text-[18px] font-semibold leading-tight tracking-[-0.01em]">"Runs"</h2>
            <span class="text-[13px] text-dim">"Grouped by workspace"</span>
        </div>
        <div class="flex flex-col gap-8">{ws_groups}</div>
    }
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let runs = RwSignal::new(Vec::<RunSummary>::new());

    // Loads (or reloads) the run list from the server. A failed poll keeps
    // whatever was last shown instead of clearing the dashboard.
    let reload_runs = move || {
        spawn_local(async move {
            if let Ok(list) = api::list_runs().await {
                runs.set(list);
            }
        });
    };

    reload_runs();
    if let Ok(handle) = set_interval_with_handle(reload_runs, POLL_INTERVAL) {
        // Stop polling once this view is torn down (the user navigated
        // away); otherwise the interval would keep firing for the rest of
        // the app's life and writing into a signal nothing reads anymore.
        on_cleanup(move || handle.clear());
    }

    view! {
        <div>
            <div class="mb-6 flex items-start justify-between">
                <div>
                    <h1 class="text-[28px] font-semibold tracking-tight">"Dashboard"</h1>
                    <p class="mt-2 text-[15px] text-muted">
                        "Every run in this session, live and finished. One run can be in flight per workspace."
                    </p>
                </div>
                <A
                    attr:class="inline-flex min-h-[38px] items-center justify-center gap-2 rounded-md bg-accent px-[18px] py-2.5 text-[14px] font-semibold text-accent-fg no-underline shadow-card transition-colors hover:bg-accent-hover active:bg-accent-press"
                    href="/workspaces"
                >
                    "+ New run"
                </A>
            </div>
            {move || {
                let list = runs.get();
                if list.is_empty() {
                    view! {
                        <div class="rounded-lg border border-dashed border-line-strong bg-surface px-6 py-12 text-center text-muted">
                            <span class="mb-3 block text-[34px] text-accent opacity-50">
                                "\u{2b21}"
                            </span>
                            <p>"No runs yet in this session."</p>
                            <A
                                attr:class="inline-flex min-h-[38px] items-center justify-center gap-2 rounded-md bg-accent px-[18px] py-2.5 text-[14px] font-semibold text-accent-fg no-underline shadow-card transition-colors hover:bg-accent-hover active:bg-accent-press"
                                href="/workspaces"
                            >
                                "Add a workspace"
                            </A>
                        </div>
                    }
                        .into_any()
                } else {
                    dashboard_body(&list).into_any()
                }
            }}
        </div>
    }
}
