//! Fetch helpers for the web UI. Every function talks to the server over a
//! same-origin relative URL and returns `Result<T, String>` so callers can
//! render the error instead of panicking. Nothing here unwraps: network
//! failures, non-success statuses, and body decoding errors are all mapped to
//! a human-readable `String`.

use gloo_net::http::Request;
use shared::{
    RefineFinalizeRequest, RefineFinalizeResponse, RefineQuestionsRequest, RefineQuestionsResponse,
    RunSummary, SaveRequest, ScanRequest, ScanResponse, StartRunRequest, StartRunResponse,
    WorkspaceDto,
};

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

/// `POST /api/runs` -> starts a pipeline run. Returns the server's error
/// message text on a 400 (validation failure) or 409 (a run is already
/// active), so the caller can show it inline.
pub async fn start_run(request: StartRunRequest) -> Result<StartRunResponse, String> {
    let response = Request::post("/api/runs")
        .json(&request)
        .map_err(|err| format!("failed to build start-run request: {err}"))?
        .send()
        .await
        .map_err(|err| format!("failed to reach the run endpoint: {err}"))?;
    into_result(response).await
}

/// `POST /api/refine/questions` -> the pass-1 refined goal, at most a
/// handful of clarifying questions, and the cost incurred.
pub async fn refine_questions(root: &str, goal: &str) -> Result<RefineQuestionsResponse, String> {
    let body = RefineQuestionsRequest {
        root: root.to_string(),
        goal: goal.to_string(),
    };
    let response = Request::post("/api/refine/questions")
        .json(&body)
        .map_err(|err| format!("failed to build refine-questions request: {err}"))?
        .send()
        .await
        .map_err(|err| format!("failed to reach the refine-questions endpoint: {err}"))?;
    into_result(response).await
}

/// `POST /api/runs/{id}/abort` -> stops the active run. Takes no body.
pub async fn abort_run(id: &str) -> Result<(), String> {
    let response = Request::post(&format!("/api/runs/{id}/abort"))
        .send()
        .await
        .map_err(|err| format!("failed to reach the abort endpoint: {err}"))?;
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("abort failed with status {status}: {body}"));
    }
    Ok(())
}

/// `POST /api/runs/{run_id}/epics/{epic_id}/retry` -> re-run one blocked epic.
/// Returns the server's error message text on a 400/404/409 so the caller can
/// show it inline. Takes no body.
pub async fn retry_epic(run_id: &str, epic_id: &str) -> Result<(), String> {
    let response = Request::post(&format!("/api/runs/{run_id}/epics/{epic_id}/retry"))
        .send()
        .await
        .map_err(|err| format!("failed to reach the retry endpoint: {err}"))?;
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("retry failed with status {status}: {body}"));
    }
    Ok(())
}

/// `POST /api/runs/{id}/resume` -> re-run every unfinished epic of a failed or
/// interrupted run. Takes no body.
pub async fn resume_run(id: &str) -> Result<(), String> {
    let response = Request::post(&format!("/api/runs/{id}/resume"))
        .send()
        .await
        .map_err(|err| format!("failed to reach the resume endpoint: {err}"))?;
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("resume failed with status {status}: {body}"));
    }
    Ok(())
}

/// `GET /api/runs` -> every run started this session (active and finished),
/// used by the app-bar runs-switcher and the dashboard.
pub async fn list_runs() -> Result<Vec<RunSummary>, String> {
    let response = Request::get("/api/runs")
        .send()
        .await
        .map_err(|err| format!("failed to fetch runs: {err}"))?;
    into_result(response).await
}

/// `POST /api/refine/finalize` -> the final refined goal folding in the
/// user's answers, and the cost incurred by this pass.
pub async fn refine_finalize(
    root: &str,
    goal: &str,
    answers: Vec<(String, String)>,
) -> Result<RefineFinalizeResponse, String> {
    let body = RefineFinalizeRequest {
        root: root.to_string(),
        goal: goal.to_string(),
        answers,
    };
    let response = Request::post("/api/refine/finalize")
        .json(&body)
        .map_err(|err| format!("failed to build refine-finalize request: {err}"))?
        .send()
        .await
        .map_err(|err| format!("failed to reach the refine-finalize endpoint: {err}"))?;
    into_result(response).await
}
