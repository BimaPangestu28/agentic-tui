//! Leptos CSR shell for the agentic-tui web UI. It is built with `trunk`
//! into `dist/`, which the server crate embeds and serves behind the
//! `--web` flag.

mod api;
mod views;
mod ws;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::path;

use views::{NewRun, Run, Workspaces};

#[component]
fn App() -> impl IntoView {
    view! {
        <Router>
            <header class="app-bar">
                <A href="/">"\u{2b21} Agentic Orchestrator"</A>
            </header>
            <main class="app-main">
                <Routes fallback=|| view! { <h1>"Not found"</h1> }>
                    <Route path=path!("/") view=Workspaces />
                    <Route path=path!("/run/new") view=NewRun />
                    <Route path=path!("/run/:id") view=Run />
                </Routes>
            </main>
        </Router>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
