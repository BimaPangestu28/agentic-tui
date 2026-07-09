//! Embedded web UI server. Serves the Leptos CSR app built by `trunk` into
//! `crates/web/dist` from memory, so the binary stays self-contained with no
//! separate static-file deployment step. Reached only behind the `--web`
//! flag; the TUI remains the default entry point.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use shared::{SaveRequest, ScanRequest, ScanResponse, WorkspaceDto};

use crate::workspace::{self, Workspace};

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

impl From<&Workspace> for WorkspaceDto {
    fn from(workspace: &Workspace) -> Self {
        WorkspaceDto {
            name: workspace.name.clone(),
            path: workspace.path.to_string_lossy().to_string(),
            base: workspace.base.clone(),
            integration: workspace.integration.clone(),
        }
    }
}

/// Turn a wire-form `WorkspaceDto` back into a native `Workspace`, expanding
/// a leading `~` the same way `--workspace <path>` does on the CLI.
fn to_workspace(dto: &WorkspaceDto) -> Workspace {
    Workspace {
        name: dto.name.clone(),
        path: workspace::expand_tilde(&dto.path),
        base: dto.base.clone(),
        integration: dto.integration.clone(),
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
        repos: repos.iter().map(WorkspaceDto::from).collect(),
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
            path: PathBuf::from("/tmp/greentic"),
            base: Some("develop".to_string()),
            integration: Some("agentic-wip".to_string()),
        };
        let dto = WorkspaceDto::from(&original);
        assert_eq!(dto.name, "greentic");
        assert_eq!(dto.path, "/tmp/greentic");
        assert_eq!(dto.base.as_deref(), Some("develop"));
        assert_eq!(dto.integration.as_deref(), Some("agentic-wip"));

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
                path: "/tmp/alpha".to_string(),
                base: Some("main".to_string()),
                integration: None,
            },
            WorkspaceDto {
                name: "beta".to_string(),
                path: "/tmp/beta".to_string(),
                base: None,
                integration: Some("agentic-wip".to_string()),
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
        assert_eq!(alpha.path, "/tmp/alpha");
        assert_eq!(alpha.base.as_deref(), Some("main"));
        let beta = listed.0.iter().find(|w| w.name == "beta").unwrap();
        assert_eq!(beta.integration.as_deref(), Some("agentic-wip"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
