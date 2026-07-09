//! End-to-end tests of the HTTP API. Each test drives the real `router()`
//! with `tower::ServiceExt::oneshot`, so routing, JSON extraction, and status
//! codes are exercised exactly as the running binary serves them, without
//! binding a socket. These complement `run_manager.rs`, which drives a full
//! pipeline run at the state level below the HTTP layer.

use agentic_tui::http::router;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

/// Read a response body to a UTF-8 string. The bodies here are small JSON or
/// HTML, so an unbounded limit is safe.
async fn body_string(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body must collect");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn the_web_ui_shell_is_served_at_the_root() {
    let response = router()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .expect("router must respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("Agentic Orchestrator"),
        "root must serve the embedded index.html shell"
    );
}

#[tokio::test]
async fn listing_runs_returns_a_json_array() {
    let response = router()
        .oneshot(
            Request::builder()
                .uri("/api/runs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.trim_start().starts_with('['),
        "GET /api/runs must return a JSON array, got: {body}"
    );
}

#[tokio::test]
async fn retrying_an_epic_of_an_unknown_run_is_404() {
    let response = router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs/no-such-run/epics/epic-1/retry")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn starting_a_run_with_no_repositories_is_400() {
    let request_body = r#"{"workspace":{"name":"empty","repos":[]},"goal":"do a thing","verify":null,"refine_cost":0.0}"#;
    let response = router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(request_body))
                .unwrap(),
        )
        .await
        .expect("router must respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_string(response).await;
    assert!(
        body.contains("at least one repository"),
        "the 400 must explain the validation failure, got: {body}"
    );
}
