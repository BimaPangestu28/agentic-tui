//! End-to-end test of the run manager (`agentic_tui::run`): starts a real
//! run against a temp git repo with a fake `claude` on `PATH`, subscribes to
//! its `App` snapshots, and asserts the stream reaches `Phase::Done` with the
//! expected cost. Also asserts `start` rejects an invalid base ref and a
//! checked-out integration target before ever touching the active-run slot.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use agentic_tui::run::{self, StartError};
use shared::{App, Phase, StartRunRequest, WorkspaceDto};
use tokio::sync::broadcast;

/// Serializes the one test here that mutates the process-wide `PATH` env var
/// (`cargo test` runs every test in this binary in one process). The other
/// two tests never spawn `claude`, so they do not need it.
static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("failed to run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A minimal git repo with one commit, so "HEAD" (and "main") resolve.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create repo dir");
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("base.txt"), "base\n").expect("write base file");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
}

/// Write a fake `claude` into `bin_dir`: it writes an empty plan (zero
/// epics, so the orchestrator has nothing to schedule and finishes right
/// after planning) to `.agentic-plan.json` in its cwd, then emits one
/// stream-json `result` line with a known cost, matching what
/// `engine::run_stage` expects from a real `claude -p --output-format
/// stream-json` invocation.
fn install_fake_claude(bin_dir: &Path) {
    std::fs::create_dir_all(bin_dir).expect("create bin dir");
    let script = bin_dir.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         echo '{\"epics\":[]}' > .agentic-plan.json\n\
         echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.25}'\n",
    )
    .expect("write fake claude script");
    let mut perms = std::fs::metadata(&script)
        .expect("stat fake claude script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod fake claude script");
}

fn workspace_dto(repo: &Path, name: &str) -> WorkspaceDto {
    WorkspaceDto {
        name: name.to_string(),
        path: repo.to_string_lossy().to_string(),
        base: None,
        integration: None,
    }
}

/// Drain `rx` into `last` until the run reaches a terminal phase or the
/// channel closes.
async fn wait_for_terminal(last: &mut App, rx: &mut broadcast::Receiver<App>) {
    while last.phase != Phase::Done && last.phase != Phase::Failed {
        match rx.recv().await {
            Ok(app) => *last = app,
            Err(_) => break,
        }
    }
}

#[tokio::test]
async fn a_run_streams_snapshots_to_done_with_the_expected_cost() {
    let _path_guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("run-mgr-happy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let bin_dir = base.join("bin");
    init_repo(&repo);
    install_fake_claude(&bin_dir);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    let request = StartRunRequest {
        workspace: workspace_dto(&repo, "happy"),
        goal: "do nothing".to_string(),
        base: None,
        into: None,
        verify: Some("true".to_string()),
        refine_cost: 0.0,
    };

    let run_id = run::start(request)
        .await
        .expect("a valid request against a real repo must be accepted");
    let (snapshot, mut rx) = run::subscribe(&run_id)
        .await
        .expect("subscribe must find the run just started");

    let mut last = snapshot;
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        wait_for_terminal(&mut last, &mut rx),
    )
    .await;

    std::env::set_var("PATH", original_path);
    let _ = std::fs::remove_dir_all(&base);

    assert!(
        result.is_ok(),
        "run did not reach a terminal phase within the timeout"
    );
    assert_eq!(
        last.phase,
        Phase::Done,
        "run should complete successfully, error: {:?}",
        last.error
    );
    assert_eq!(last.total_cost, 0.25);
}

#[tokio::test]
async fn start_rejects_an_invalid_base_ref() {
    let base = std::env::temp_dir().join(format!("run-mgr-badbase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    init_repo(&base);

    let request = StartRunRequest {
        workspace: workspace_dto(&base, "badbase"),
        goal: "irrelevant".to_string(),
        base: Some("no-such-branch".to_string()),
        into: None,
        verify: None,
        refine_cost: 0.0,
    };

    let err = run::start(request)
        .await
        .expect_err("an unresolvable base ref must be rejected");
    assert!(matches!(err, StartError::Invalid(_)));

    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn start_rejects_a_checked_out_integration_target() {
    let base = std::env::temp_dir().join(format!("run-mgr-checkedout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    init_repo(&base);

    // `init_repo` leaves "main" checked out, so targeting it as the
    // integration branch must be rejected: merges into it would need a
    // worktree of a branch already checked out in the main working tree.
    let request = StartRunRequest {
        workspace: workspace_dto(&base, "checkedout"),
        goal: "irrelevant".to_string(),
        base: None,
        into: Some("main".to_string()),
        verify: None,
        refine_cost: 0.0,
    };

    let err = run::start(request)
        .await
        .expect_err("an integration target checked out in the workspace must be rejected");
    assert!(matches!(err, StartError::Invalid(_)));

    let _ = std::fs::remove_dir_all(&base);
}
