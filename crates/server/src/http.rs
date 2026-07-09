//! Embedded web UI server. Serves the Leptos CSR app built by `trunk` into
//! `crates/web/dist` from memory, so the binary stays self-contained with no
//! separate static-file deployment step. Reached only behind the `--web`
//! flag; the TUI remains the default entry point.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

/// Serve an embedded asset for any path, falling back to `index.html` so
/// client-side routes (once the app has any) resolve to the same shell.
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| Assets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Build the router for the embedded web UI.
pub fn router() -> Router {
    Router::new().fallback(static_handler)
}

/// Bind loopback on an ephemeral port, print the URL, optionally open the
/// default browser, and serve until the process is stopped.
pub async fn serve(open_browser: bool) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    println!("agentic-tui web UI at {url}");
    if open_browser {
        if let Err(e) = open::that(&url) {
            eprintln!("warning: could not open browser: {e}");
        }
    }
    axum::serve(listener, router()).await?;
    Ok(())
}
