//! Embedded web UI server. Serves the Leptos CSR app built by `trunk` into
//! `crates/web/dist` from memory, so the binary stays self-contained with no
//! separate static-file deployment step. This is the default and only UI: the
//! binary starts this server and opens the browser.

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::Path,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use shared::{
    RefineFinalizeRequest, RefineFinalizeResponse, RefineQuestionsRequest, RefineQuestionsResponse,
    RepoDto, RunSummary, SaveRequest, ScanRequest, ScanResponse, StartRunRequest, StartRunResponse,
    WorkspaceDto,
};
use tokio::sync::broadcast;

use crate::refine;
use crate::run::{self, StartError};
use crate::workspace::{self, Repo, Workspace};

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

impl From<&Repo> for RepoDto {
    fn from(repo: &Repo) -> Self {
        RepoDto {
            name: repo.name.clone(),
            path: repo.path.to_string_lossy().to_string(),
            base: repo.base.clone(),
            integration: repo.integration.clone(),
        }
    }
}

impl From<&Workspace> for WorkspaceDto {
    fn from(workspace: &Workspace) -> Self {
        WorkspaceDto {
            name: workspace.name.clone(),
            repos: workspace.repos.iter().map(RepoDto::from).collect(),
        }
    }
}

/// Turn a wire-form `RepoDto` back into a native `Repo`, expanding a leading
/// `~` in its path the same way `--workspace <path>` does on the CLI.
fn to_repo(dto: &RepoDto) -> Repo {
    Repo {
        name: dto.name.clone(),
        path: workspace::expand_tilde(&dto.path),
        base: dto.base.clone(),
        integration: dto.integration.clone(),
    }
}

/// Turn a wire-form `WorkspaceDto` back into a native `Workspace`, expanding a
/// leading `~` in every repo path.
fn to_workspace(dto: &WorkspaceDto) -> Workspace {
    Workspace {
        name: dto.name.clone(),
        repos: dto.repos.iter().map(to_repo).collect(),
    }
}

/// `GET /api/workspaces`: the saved workspace list, empty when the config
/// file is missing or unreadable.
async fn list_workspaces() -> Json<Vec<WorkspaceDto>> {
    let workspaces =
        workspace::load_workspaces(&workspace::default_config_path()).unwrap_or_default();
    Json(workspaces.iter().map(WorkspaceDto::from).collect())
}

/// `POST /api/workspaces/scan`: repos found under the requested root.
async fn scan_workspaces(Json(request): Json<ScanRequest>) -> Json<ScanResponse> {
    let root = workspace::expand_tilde(&request.root);
    let repos = workspace::scan_for_repos(&root, workspace::DEFAULT_SCAN_DEPTH);
    Json(ScanResponse {
        repos: repos.iter().map(RepoDto::from).collect(),
    })
}

/// `POST /api/workspaces`: persist the given list, merging with any entries
/// already saved. Returns 200 on success, or 500 with the error text.
async fn save_workspaces_handler(Json(request): Json<SaveRequest>) -> Response {
    let workspaces: Vec<Workspace> = request.workspaces.iter().map(to_workspace).collect();
    match workspace::save_workspaces(&workspace::default_config_path(), &workspaces) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/runs`: every run started this session (active and finished),
/// for the multi-run dashboard.
async fn list_runs() -> Json<Vec<RunSummary>> {
    Json(run::list().await)
}

/// `POST /api/runs`: start a pipeline run. 400 with a message if the request
/// does not resolve to a runnable repo/refs, 409 if the request's workspace
/// already has a run in flight.
async fn start_run(Json(request): Json<StartRunRequest>) -> Response {
    match run::start(request).await {
        Ok(run_id) => Json(StartRunResponse { run_id }).into_response(),
        Err(e @ StartError::WorkspaceBusy) => (StatusCode::CONFLICT, e.message()).into_response(),
        Err(e @ StartError::Invalid(_)) => (StatusCode::BAD_REQUEST, e.message()).into_response(),
    }
}

/// `POST /api/runs/{id}/abort`: abort the run if it is the active one and has
/// not completed. A no-op (still 200) for an unknown or finished id.
async fn abort_run(Path(id): Path<String>) -> StatusCode {
    run::abort(&id).await;
    StatusCode::OK
}

/// `GET /api/runs/{id}/events`: upgrade to a WebSocket, send the current
/// `App` snapshot as JSON text, then forward every broadcast snapshot as JSON
/// text until the channel closes.
async fn run_events(Path(id): Path<String>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| stream_run(socket, id))
}

async fn stream_run(mut socket: WebSocket, id: String) {
    // Returning drops `socket`, which closes the connection; there is no
    // separate close handshake to send beyond that.
    let Some((snapshot, mut rx)) = run::subscribe(&id).await else {
        return;
    };
    let Ok(text) = serde_json::to_string(&snapshot) else {
        return;
    };
    if socket.send(Message::Text(text.into())).await.is_err() {
        return;
    }

    loop {
        match rx.recv().await {
            Ok(app) => {
                let Ok(text) = serde_json::to_string(&app) else {
                    continue;
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            // A slow subscriber that fell behind: resync on the next snapshot
            // rather than closing the connection.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// `POST /api/refine/questions`: run the first refine pass, rewriting the
/// goal and gathering clarifying questions. Falls back to the original goal
/// with no questions if the pass fails; the incurred cost is reported
/// either way.
async fn refine_questions(
    Json(request): Json<RefineQuestionsRequest>,
) -> Json<RefineQuestionsResponse> {
    let root = workspace::expand_tilde(&request.root);
    let (refined_goal, questions, cost) = refine::questions(&root, &request.goal).await;
    Json(RefineQuestionsResponse {
        refined_goal,
        questions,
        cost,
    })
}

/// `POST /api/refine/finalize`: run the second refine pass, folding the
/// user's answers into a final goal. Falls back to the original goal if the
/// pass fails; the incurred cost is reported either way.
async fn refine_finalize(
    Json(request): Json<RefineFinalizeRequest>,
) -> Json<RefineFinalizeResponse> {
    let root = workspace::expand_tilde(&request.root);
    let (refined_goal, cost) = refine::finalize(&root, &request.goal, &request.answers).await;
    Json(RefineFinalizeResponse { refined_goal, cost })
}

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

/// Build the router for the embedded web UI, mounting the workspace API
/// before the static asset fallback.
pub fn router() -> Router {
    Router::new()
        .route(
            "/api/workspaces",
            get(list_workspaces).post(save_workspaces_handler),
        )
        .route("/api/workspaces/scan", post(scan_workspaces))
        .route("/api/runs", get(list_runs).post(start_run))
        .route("/api/runs/{id}/abort", post(abort_run))
        .route("/api/runs/{id}/events", get(run_events))
        .route("/api/refine/questions", post(refine_questions))
        .route("/api/refine/finalize", post(refine_finalize))
        .fallback(static_handler)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn dto_conversion_round_trips_a_workspace() {
        let original = Workspace {
            name: "greentic".to_string(),
            repos: vec![Repo {
                name: "greentic".to_string(),
                path: PathBuf::from("/tmp/greentic"),
                base: Some("develop".to_string()),
                integration: Some("agentic-wip".to_string()),
            }],
        };
        let dto = WorkspaceDto::from(&original);
        assert_eq!(dto.name, "greentic");
        assert_eq!(dto.repos.len(), 1);
        assert_eq!(dto.repos[0].path, "/tmp/greentic");
        assert_eq!(dto.repos[0].base.as_deref(), Some("develop"));
        assert_eq!(dto.repos[0].integration.as_deref(), Some("agentic-wip"));

        let back = to_workspace(&dto);
        assert_eq!(back, original);
    }

    #[tokio::test]
    async fn scan_handler_finds_a_git_repo_as_a_dto() {
        let root = std::env::temp_dir().join(format!("http-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repoA/.git")).unwrap();

        let response = scan_workspaces(Json(ScanRequest {
            root: root.to_string_lossy().to_string(),
        }))
        .await;

        let found = response
            .0
            .repos
            .iter()
            .find(|dto| dto.path == root.join("repoA").to_string_lossy());
        assert!(
            found.is_some(),
            "scan handler must report the repo as a WorkspaceDto"
        );
        assert_eq!(found.unwrap().name, "repoA");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn save_then_list_round_trips_through_the_dto_boundary() {
        let _guard = workspace::HOME_ENV_LOCK.lock().await;
        let home = std::env::temp_dir().join(format!("http-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);

        let workspaces = vec![
            WorkspaceDto {
                name: "alpha".to_string(),
                repos: vec![RepoDto {
                    name: "alpha".to_string(),
                    path: "/tmp/alpha".to_string(),
                    base: Some("main".to_string()),
                    integration: None,
                }],
            },
            WorkspaceDto {
                name: "beta".to_string(),
                repos: vec![RepoDto {
                    name: "beta".to_string(),
                    path: "/tmp/beta".to_string(),
                    base: None,
                    integration: Some("agentic-wip".to_string()),
                }],
            },
        ];

        let save_response = save_workspaces_handler(Json(SaveRequest {
            workspaces: workspaces.clone(),
        }))
        .await;
        assert_eq!(save_response.status(), StatusCode::OK);

        let listed = list_workspaces().await;
        assert_eq!(listed.0.len(), 2, "list must reflect the saved entries");
        let alpha = listed.0.iter().find(|w| w.name == "alpha").unwrap();
        assert_eq!(alpha.repos[0].path, "/tmp/alpha");
        assert_eq!(alpha.repos[0].base.as_deref(), Some("main"));
        let beta = listed.0.iter().find(|w| w.name == "beta").unwrap();
        assert_eq!(beta.repos[0].integration.as_deref(), Some("agentic-wip"));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Serializes the refine-endpoint tests below, which all mutate the
    /// process-wide `PATH` env var to point a fake `claude` at a script that
    /// writes `.agentic-refine.json` (`cargo test` runs every test in this
    /// binary in one process, so unguarded mutation would race).
    static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Write a fake `claude` into `bin_dir` running `script_body`, matching
    /// what `engine::run_stage` expects: a `claude -p ... --output-format
    /// stream-json` process whose stdout is NDJSON.
    fn install_fake_claude(bin_dir: &std::path::Path, script_body: &str) {
        std::fs::create_dir_all(bin_dir).unwrap();
        let script = bin_dir.join("claude");
        std::fs::write(&script, script_body).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }

    /// Point `PATH` at `bin_dir` (prepended to the current `PATH`) for the
    /// duration of the caller's fake-`claude` test, returning the original
    /// value to restore afterward.
    fn prepend_path(bin_dir: &std::path::Path) -> String {
        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));
        original_path
    }

    #[tokio::test]
    async fn refine_questions_handler_returns_the_parsed_goal_and_truncates_questions() {
        let _path_guard = PATH_ENV_LOCK.lock().await;

        let base = std::env::temp_dir().join(format!("http-refine-q-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&repo).unwrap();
        install_fake_claude(
            &bin_dir,
            "#!/bin/sh\n\
             echo '{\"refined_goal\":\"add a health check endpoint at /healthz\",\
             \"questions\":[\"Which port?\",\"Auth required?\",\"Which framework?\",\
             \"Should it check DB connectivity?\",\"JSON or plain text?\",\
             \"Which HTTP method?\",\"Add a version endpoint too?\"]}' > .agentic-refine.json\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.03}'\n",
        );
        let original_path = prepend_path(&bin_dir);

        let response = refine_questions(Json(RefineQuestionsRequest {
            root: repo.to_string_lossy().to_string(),
            goal: "add a health check".to_string(),
        }))
        .await;

        std::env::set_var("PATH", original_path);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(
            response.0.refined_goal,
            "add a health check endpoint at /healthz"
        );
        assert_eq!(
            response.0.questions.len(),
            crate::config::REFINE_MAX_QUESTIONS
        );
        assert_eq!(response.0.questions[4], "JSON or plain text?");
        assert_eq!(response.0.cost, 0.03);
    }

    #[tokio::test]
    async fn refine_finalize_handler_returns_the_final_goal() {
        let _path_guard = PATH_ENV_LOCK.lock().await;

        let base = std::env::temp_dir().join(format!("http-refine-f-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&repo).unwrap();
        install_fake_claude(
            &bin_dir,
            "#!/bin/sh\n\
             echo '{\"refined_goal\":\"add a health check endpoint at /healthz on port 8080\",\
             \"questions\":[]}' > .agentic-refine.json\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.02}'\n",
        );
        let original_path = prepend_path(&bin_dir);

        let response = refine_finalize(Json(RefineFinalizeRequest {
            root: repo.to_string_lossy().to_string(),
            goal: "add a health check".to_string(),
            answers: vec![("Which port?".to_string(), "8080".to_string())],
        }))
        .await;

        std::env::set_var("PATH", original_path);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(
            response.0.refined_goal,
            "add a health check endpoint at /healthz on port 8080"
        );
        assert_eq!(response.0.cost, 0.02);
    }

    #[tokio::test]
    async fn a_billed_but_unparseable_refine_pass_still_reports_cost_and_falls_back() {
        let _path_guard = PATH_ENV_LOCK.lock().await;

        let base = std::env::temp_dir().join(format!("http-refine-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&repo).unwrap();
        install_fake_claude(
            &bin_dir,
            "#!/bin/sh\n\
             echo 'not json at all' > .agentic-refine.json\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.05}'\n",
        );
        let original_path = prepend_path(&bin_dir);

        let response = refine_questions(Json(RefineQuestionsRequest {
            root: repo.to_string_lossy().to_string(),
            goal: "add a health check".to_string(),
        }))
        .await;

        std::env::set_var("PATH", original_path);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(response.0.refined_goal, "add a health check");
        assert!(response.0.questions.is_empty());
        assert_eq!(response.0.cost, 0.05);
    }
}
