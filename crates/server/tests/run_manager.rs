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
use shared::{App, Phase, RepoDto, StartRunRequest, WorkspaceDto};
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
        repos: vec![RepoDto {
            name: name.to_string(),
            path: repo.to_string_lossy().to_string(),
            base: None,
            integration: None,
        }],
    }
}

/// Write a fake `claude` that sleeps before finishing, so a run started
/// against it stays active (not yet `completed`) long enough for a test to
/// exercise the per-workspace busy check while it is still in flight.
fn install_slow_fake_claude(bin_dir: &Path, sleep_secs: u32) {
    std::fs::create_dir_all(bin_dir).expect("create bin dir");
    let script = bin_dir.join("claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             sleep {sleep_secs}\n\
             echo '{{\"epics\":[]}}' > .agentic-plan.json\n\
             echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.1}}'\n"
        ),
    )
    .expect("write fake claude script");
    let mut perms = std::fs::metadata(&script)
        .expect("stat fake claude script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod fake claude script");
}

/// The registry rejects a second run for a workspace that already has one
/// active, allows a run in a different workspace, `list()` reports both, and
/// an aborted run stays listed with a terminal phase instead of disappearing.
#[tokio::test]
async fn registry_is_busy_per_workspace_and_keeps_aborted_runs_listed() {
    let _path_guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("run-mgr-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo_a = base.join("repo-a");
    let repo_b = base.join("repo-b");
    let bin_dir = base.join("bin");
    init_repo(&repo_a);
    init_repo(&repo_b);
    // Sleep long enough that the busy check below and the abort still see it
    // as active, without dragging the test out.
    install_slow_fake_claude(&bin_dir, 5);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    let request_a = StartRunRequest {
        workspace: workspace_dto(&repo_a, "regA"),
        goal: "do nothing".to_string(),
        verify: Some("true".to_string()),
        refine_cost: 0.0,
        language: shared::Language::English,
    };
    let run_a = run::start(request_a)
        .await
        .expect("the first run in workspace regA must be accepted");

    // Same workspace, still active: rejected.
    let request_a_again = StartRunRequest {
        workspace: workspace_dto(&repo_a, "regA"),
        goal: "do something else".to_string(),
        verify: Some("true".to_string()),
        refine_cost: 0.0,
        language: shared::Language::English,
    };
    let err = run::start(request_a_again)
        .await
        .expect_err("a second run in the same active workspace must be rejected");
    assert_eq!(err, StartError::WorkspaceBusy);

    // A different workspace is unaffected.
    let request_b = StartRunRequest {
        workspace: workspace_dto(&repo_b, "regB"),
        goal: "do nothing".to_string(),
        verify: Some("true".to_string()),
        refine_cost: 0.0,
        language: shared::Language::English,
    };
    let run_b = run::start(request_b)
        .await
        .expect("a run in a different workspace must be accepted");

    let listed = run::list().await;
    assert!(
        listed
            .iter()
            .any(|s| s.id == run_a && s.workspace == "regA"),
        "list() must include the run started in workspace regA"
    );
    assert!(
        listed
            .iter()
            .any(|s| s.id == run_b && s.workspace == "regB"),
        "list() must include the run started in workspace regB"
    );

    run::abort(&run_a).await;

    let listed_after_abort = run::list().await;
    let summary_a = listed_after_abort
        .iter()
        .find(|s| s.id == run_a)
        .expect("aborted run must still appear in list()");
    assert_eq!(
        summary_a.phase,
        shared::Phase::Failed,
        "an aborted run must show a terminal phase"
    );

    std::env::set_var("PATH", original_path);
    let _ = std::fs::remove_dir_all(&base);
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
        verify: Some("true".to_string()),
        refine_cost: 0.0,
        language: shared::Language::English,
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

    let mut workspace = workspace_dto(&base, "badbase");
    workspace.repos[0].base = Some("no-such-branch".to_string());
    let request = StartRunRequest {
        workspace,
        goal: "irrelevant".to_string(),
        verify: None,
        refine_cost: 0.0,
        language: shared::Language::English,
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
    let mut workspace = workspace_dto(&base, "checkedout");
    workspace.repos[0].integration = Some("main".to_string());
    let request = StartRunRequest {
        workspace,
        goal: "irrelevant".to_string(),
        verify: None,
        refine_cost: 0.0,
        language: shared::Language::English,
    };

    let err = run::start(request)
        .await
        .expect_err("an integration target checked out in the workspace must be rejected");
    assert!(matches!(err, StartError::Invalid(_)));

    let _ = std::fs::remove_dir_all(&base);
}
