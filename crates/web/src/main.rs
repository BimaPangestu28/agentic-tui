//! Leptos CSR shell for the agentic-tui web UI. It is built with `trunk`
//! into `dist/`, which the server crate embeds and serves behind the
//! `--web` flag.

mod api;
mod views;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use views::Workspaces;

/// Placeholder for the new-run wizard, wired up in a later task.
#[component]
fn NewRun() -> impl IntoView {
    view! { <h1>"New run"</h1> }
}

/// Placeholder for the run dashboard, wired up in a later task.
#[component]
fn Run() -> impl IntoView {
    view! { <h1>"Run"</h1> }
}

#[component]
fn App() -> impl IntoView {
    view! {
        <Router>
            <Routes fallback=|| view! { <h1>"Not found"</h1> }>
                <Route path=path!("/") view=Workspaces />
                <Route path=path!("/run/new") view=NewRun />
                <Route path=path!("/run/:id") view=Run />
            </Routes>
        </Router>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
