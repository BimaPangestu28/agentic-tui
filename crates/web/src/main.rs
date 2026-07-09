//! Minimal Leptos CSR shell for the agentic-tui web UI. This crate mounts a
//! single heading today; it does not yet connect to the server. It is built
//! with `trunk` into `dist/`, which the server crate embeds and serves
//! behind the `--web` flag.

use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(|| view! { <h1>"Agentic Orchestrator"</h1> });
}
