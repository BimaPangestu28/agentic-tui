//! WebSocket client for a run's live `App` snapshots. The server sends the
//! current `App` as JSON text as soon as the socket opens, then every
//! updated snapshot afterward; this module just decodes each message and
//! pushes it into the caller's signal.

use leptos::prelude::*;
use shared::App;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// Builds the events URL for a run, deriving the scheme and host from the
/// current page location so it works no matter what host/port the server is
/// reachable on (`ws://` for an `http:` page, `wss://` for `https:`).
fn events_url(run_id: &str) -> Option<String> {
    let window = web_sys::window()?;
    let location = window.location();
    let protocol = location.protocol().ok()?;
    let host = location.host().ok()?;
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    Some(format!("{scheme}://{host}/api/runs/{run_id}/events"))
}

/// Opens the WebSocket for `run_id` and, on every text message, decodes it
/// as an `App` and writes it into `app`. A message that fails to decode is
/// skipped rather than treated as fatal, since one malformed frame should
/// not take down an otherwise-live view.
///
/// Returns the socket handle on success, or `None` if the page location or
/// the socket itself could not be obtained. The caller must hold on to the
/// returned handle for as long as the connection should stay open; dropping
/// it closes the underlying connection.
pub fn connect(run_id: &str, app: RwSignal<Option<App>>) -> Option<WebSocket> {
    let url = events_url(run_id)?;
    let socket = WebSocket::new(&url).ok()?;

    let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(text) = event.data().as_string() else {
            return;
        };
        if let Ok(snapshot) = serde_json::from_str::<App>(&text) {
            app.set(Some(snapshot));
        }
    });
    socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    // The handler must outlive this function call; it is dropped only when
    // the socket itself closes and is torn down (there is no owning place
    // to stash it once `connect` returns, so we deliberately leak it).
    onmessage.forget();

    Some(socket)
}
