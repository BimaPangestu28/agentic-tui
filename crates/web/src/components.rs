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

    // Tailwind utilities. `nav-link` / `chip` naming kept only where a shared
    // look repeats; everything else is inline utilities. The runs menu opens on
    // `:focus-within` (the trigger is a real button), mirrored with `group` +
    // `group-focus-within`.
    let nav_link = "inline-flex items-center rounded-md px-3 py-1.5 text-[13px] \
                    font-medium text-muted no-underline transition-colors \
                    hover:bg-inset hover:text-ink";
    view! {
        <header class="sticky top-0 z-50 flex h-[52px] items-center gap-3 border-b border-line bg-page/80 px-6 backdrop-blur-md backdrop-saturate-150">
            <A attr:class="inline-flex items-center gap-2 text-[15px] font-semibold tracking-tight text-ink no-underline" href="/">
                <span class="text-[18px] leading-none text-accent">"\u{2b21}"</span>
                " Agentic Orchestrator"
            </A>
            <nav class="ml-auto flex items-center gap-1">
                <A attr:class=nav_link href="/workspaces">
                    "Workspaces"
                </A>
                <A attr:class=nav_link href="/workspaces">
                    "New run"
                </A>
            </nav>
            <div class="group relative">
                <button
                    type="button"
                    class="inline-flex items-center gap-2 rounded-full border border-line-strong bg-inset px-3 py-1.5 text-[13px] font-medium text-ink transition-colors hover:bg-raised"
                >
                    <Show when=move || { active_count() > 0 }>
                        <span class="size-2 animate-pulse rounded-full bg-prog"></span>
                    </Show>
                    <span>{move || format!("{} running", active_count())}</span>
                    <span class="text-[10px] text-dim">"\u{25be}"</span>
                </button>
                <div class="absolute right-0 top-[calc(100%+8px)] z-[60] hidden w-[340px] rounded-lg border border-line-strong bg-surface p-2 shadow-float group-focus-within:block">
                    <div class="flex items-center justify-between px-3 pb-3 pt-2 text-[12px] font-bold uppercase tracking-wider text-dim">
                        <span>"Active runs"</span>
                        <A attr:class=nav_link href="/">
                            "View all"
                        </A>
                    </div>
                    <For
                        each=active_runs
                        key=|run| run.id.clone()
                        children=move |run: RunSummary| {
                            let href = format!("/run/{}", run.id);
                            view! {
                                <A
                                    attr:class="grid grid-cols-[auto_1fr_auto] items-center gap-3 rounded-md p-3 text-inherit no-underline hover:bg-inset"
                                    href=href
                                >
                                    <span class="size-2.5 animate-pulse rounded-full bg-prog"></span>
                                    <span class="min-w-0">
                                        <span class="font-mono text-[13px] font-semibold text-ink">
                                            {run.workspace.clone()}
                                            <span class="font-medium text-dim">
                                                {format!(" \u{00b7} {} repos", run.repos.len())}
                                            </span>
                                        </span>
                                        <span class="block truncate text-[12px] text-dim">
                                            {run.goal.clone()}
                                        </span>
                                    </span>
                                    <span class="font-mono text-[12px] text-muted">
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
