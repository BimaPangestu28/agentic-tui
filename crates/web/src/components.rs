//! Shared chrome for every view: the app bar, its nav links, and the
//! runs-switcher dropdown that shows what is active right now.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use shared::{Phase, RunSummary};

use crate::api;

/// How often the runs-switcher polls `GET /api/runs` for fresh phases and
/// budgets. Short enough to feel live, long enough to stay cheap.
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// The persistent header shown above every route: the brand mark, primary
/// nav, and the runs-switcher. Mounted once outside `<Routes>` in `main.rs`.
#[component]
pub fn AppBar() -> impl IntoView {
    let runs = RwSignal::new(Vec::<RunSummary>::new());

    // Loads (or reloads) the run list from the server. A failed poll keeps
    // whatever was last shown instead of clearing the menu.
    let reload_runs = move || {
        spawn_local(async move {
            if let Ok(list) = api::list_runs().await {
                runs.set(list);
            }
        });
    };

    reload_runs();
    set_interval(reload_runs, POLL_INTERVAL);

    // Runs currently in flight: planning or implementing. Everything else
    // (done, failed) is left for the dashboard, not the switcher.
    let active_runs = move || -> Vec<RunSummary> {
        runs.get()
            .into_iter()
            .filter(|run| matches!(run.phase, Phase::Planning | Phase::Implementing))
            .collect()
    };
    let active_count = move || active_runs().len();

    view! {
        <header class="app-bar">
            <A href="/">
                <span class="hex">"\u{2b21}"</span>
                " Agentic Orchestrator"
            </A>
            <nav style="margin-left: auto;">
                <A attr:class="btn btn-ghost btn-sm" href="/workspaces">
                    "Workspaces"
                </A>
                <A attr:class="btn btn-ghost btn-sm" href="/workspaces">
                    "New run"
                </A>
            </nav>
            <div class="runs-switcher">
                <button type="button" class="trigger">
                    <Show when=move || { active_count() > 0 }>
                        <span class="live-dot"></span>
                    </Show>
                    <span>{move || format!("{} running", active_count())}</span>
                    <span class="caret">"\u{25be}"</span>
                </button>
                <div class="runs-menu">
                    <div class="runs-menu-head">
                        <span>"Active runs"</span>
                        <A attr:class="btn btn-ghost btn-sm" href="/">
                            "View all"
                        </A>
                    </div>
                    <For
                        each=active_runs
                        key=|run| run.id.clone()
                        children=move |run: RunSummary| {
                            let href = format!("/run/{}", run.id);
                            view! {
                                <A attr:class="runs-menu-item" href=href>
                                    <span class="phase-dot"></span>
                                    <span class="col">
                                        <span class="ws">
                                            {run.workspace.clone()}
                                            <span class="ws-repos">
                                                {format!(" \u{00b7} {} repos", run.repos.len())}
                                            </span>
                                        </span>
                                        <span class="goal-snip">{run.goal.clone()}</span>
                                    </span>
                                    <span class="mini-budget">
                                        {format!("${:.2}", run.total_cost)}
                                    </span>
                                </A>
                            }
                        }
                    />
                </div>
            </div>
        </header>
    }
}
