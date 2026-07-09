//! Leptos CSR shell for the agentic-tui web UI. It is built with `trunk`
//! into `dist/`, which the server crate embeds and serves behind the
//! `--web` flag.

mod api;
mod components;
mod views;
mod ws;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use components::AppBar;
use views::{Dashboard, NewRun, Run, Workspaces};

#[component]
fn App() -> impl IntoView {
    view! {
        <Router>
            <AppBar />
            <main class="app-main">
                <Routes fallback=|| view! { <h1>"Not found"</h1> }>
                    <Route path=path!("/") view=Dashboard />
                    <Route path=path!("/workspaces") view=Workspaces />
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
