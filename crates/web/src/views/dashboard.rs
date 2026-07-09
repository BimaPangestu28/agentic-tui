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

/// Phase -> the CSS modifier class used on phase dots, phase badges, and
/// run-card borders. `Phase` has no `Running` variant; `Implementing` is
/// the phase shown to the user as "Running".
fn phase_class(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => "planning",
        Phase::Implementing => "running",
        Phase::Done => "done",
        Phase::Failed => "failed",
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

/// Kanban column -> the CSS suffix used by `.seg`/`.sw` in the status
/// distribution bar and legend (todo/prog/review/done/block).
fn column_class(column: KanbanColumn) -> &'static str {
    match column {
        KanbanColumn::Todo => "todo",
        KanbanColumn::InProgress => "prog",
        KanbanColumn::Review => "review",
        KanbanColumn::Done => "done",
        KanbanColumn::Blocked => "block",
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
    // for the global board's `.kanban-card-run` tag.
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
            view! { <div class=format!("seg {class}") style=format!("width: {pct}%")></div> }
        })
        .collect();

    let legend: Vec<_> = column_counts
        .iter()
        .map(|(_, label, class, count)| {
            view! {
                <span class="legend-item">
                    <span class=format!("sw {class}")></span>
                    {*label}
                    <span class="n">{*count}</span>
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
                        <div class="kanban-card">
                            <div class="kanban-card-run">{workspace.clone()}</div>
                            <div class="kanban-card-title">{epic.title.clone()}</div>
                            <div class="kanban-card-meta">
                                <span class="kanban-card-id">{epic.id.clone()}</span>
                                <span class="kanban-card-status">
                                    {status_label(epic.status)}
                                </span>
                            </div>
                        </div>
                    }
                })
                .collect();
            view! {
                <div class="kanban-column">
                    <h3>{*label} " " <span class="count">{*count}</span></h3>
                    <div class="kanban-cards">{cards}</div>
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
                // Summarize the group's repos: the single repo's name, or a
                // count when the run spans several.
                let repo_summary = match run.repos.as_slice() {
                    [only] => only.clone(),
                    repos => format!("{} repos", repos.len()),
                };
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
                    let class = phase_class(run.phase);
                    let href = format!("/run/{}", run.id);
                    view! {
                        <A attr:class=format!("run-card {class}") href=href>
                            <span class="phase-dot"></span>
                            <div class="col">
                                <div class="rc-goal">{run.goal.clone()}</div>
                                <div class="rc-meta">
                                    {format!(
                                        "{} epics \u{00b7} {} repos",
                                        run.epics.len(),
                                        run.repos.len(),
                                    )}
                                </div>
                            </div>
                            <div class="rc-right">
                                <span class=format!("run-phase {class}")>
                                    {phase_label(run.phase)}
                                </span>
                                <span class="mini-budget">
                                    {format!("${:.2}", run.total_cost)}
                                </span>
                            </div>
                        </A>
                    }
                })
                .collect();
            view! {
                <div class="ws-group">
                    <div class="ws-group-head">
                        <span class="ws-hex">"\u{2b21}"</span>
                        <span class="ws-name">{name}</span>
                        <span class="ws-path">{repo_summary}</span>
                        <span class="spacer"></span>
                        <span class="ws-count">{format!("{count} runs")}</span>
                        <A attr:class="btn btn-ghost btn-sm" href=new_run_href>
                            "+ New run"
                        </A>
                    </div>
                    <div class="runs-list">{cards}</div>
                </div>
            }
        })
        .collect();

    view! {
        <div class="dashboard-overview">
            <div class="overview-card">
                <div class="overview-stats">
                    <div class="ov-stat">
                        <span class="ov-label">"Active loops"</span>
                        <span class="ov-value">
                            {active_runs}
                            " "
                            <span class="dim">{format!("/ {total_runs}")}</span>
                        </span>
                    </div>
                    <div class="ov-stat">
                        <span class="ov-label">"Epics"</span>
                        <span class="ov-value">{total_epics}</span>
                    </div>
                    <div class="ov-stat">
                        <span class="ov-label">"Total spend"</span>
                        <span class="ov-value mono">{format!("${total_spend:.2}")}</span>
                    </div>
                </div>
                <div class="overview-divider"></div>
                <div class="overview-dist">
                    <div class="dist-title">"Epic status"</div>
                    <div class="stacked-bar">{segs}</div>
                    <div class="chart-legend">{legend}</div>
                </div>
            </div>
        </div>

        <div class="section-head">
            <h2>"Board"</h2>
            <span class="section-sub">"Every epic, across every run"</span>
        </div>
        <div class="kanban-board">{board_columns}</div>

        <div class="section-head">
            <h2>"Runs"</h2>
            <span class="section-sub">"Grouped by workspace"</span>
        </div>
        <div class="ws-groups">{ws_groups}</div>
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
        <div class="dashboard-view">
            <div class="page-head" style="display: flex; justify-content: space-between; align-items: flex-start;">
                <div>
                    <h1>"Dashboard"</h1>
                    <p class="sub">
                        "Every run in this session, live and finished. One run can be in flight per workspace."
                    </p>
                </div>
                <A attr:class="btn btn-primary" href="/workspaces">
                    "+ New run"
                </A>
            </div>
            {move || {
                let list = runs.get();
                if list.is_empty() {
                    view! {
                        <div class="empty-state">
                            <span class="hex">"\u{2b21}"</span>
                            <p>"No runs yet in this session."</p>
                            <A attr:class="btn btn-primary" href="/workspaces">
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
