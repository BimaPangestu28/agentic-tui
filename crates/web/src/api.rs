//! Fetch helpers for the web UI. Every function talks to the server over a
//! same-origin relative URL and returns `Result<T, String>` so callers can
//! render the error instead of panicking. Nothing here unwraps: network
//! failures, non-success statuses, and body decoding errors are all mapped to
//! a human-readable `String`.

use gloo_net::http::Request;
use shared::{SaveRequest, ScanRequest, ScanResponse, WorkspaceDto};

/// Reads the response body as `T` when the status is a success code, or
/// builds an error string from the status and body text otherwise.
async fn into_result<T: serde::de::DeserializeOwned>(
    response: gloo_net::http::Response,
) -> Result<T, String> {
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed with status {status}: {body}"));
    }
    response
        .json::<T>()
        .await
        .map_err(|err| format!("failed to decode response: {err}"))
}

/// `GET /api/workspaces` -> the configured workspace list.
pub async fn list_workspaces() -> Result<Vec<WorkspaceDto>, String> {
    let response = Request::get("/api/workspaces")
        .send()
        .await
        .map_err(|err| format!("failed to fetch workspaces: {err}"))?;
    into_result(response).await
}

/// `POST /api/workspaces/scan` -> repos found under `root`.
pub async fn scan(root: &str) -> Result<ScanResponse, String> {
    let body = ScanRequest {
        root: root.to_string(),
    };
    let response = Request::post("/api/workspaces/scan")
        .json(&body)
        .map_err(|err| format!("failed to build scan request: {err}"))?
        .send()
        .await
        .map_err(|err| format!("failed to reach scan endpoint: {err}"))?;
    into_result(response).await
}

/// `POST /api/workspaces` -> persists the full workspace list.
pub async fn save(workspaces: &[WorkspaceDto]) -> Result<(), String> {
    let body = SaveRequest {
        workspaces: workspaces.to_vec(),
    };
    let response = Request::post("/api/workspaces")
        .json(&body)
        .map_err(|err| format!("failed to build save request: {err}"))?
        .send()
        .await
        .map_err(|err| format!("failed to reach save endpoint: {err}"))?;
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("save failed with status {status}: {body}"));
    }
    Ok(())
}
