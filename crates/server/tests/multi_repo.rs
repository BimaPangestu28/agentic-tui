//! A run whose plan spans two repos merges each epic into its own repo's
//! integration branch. Exercises the engine's multi-repo path directly at the
//! orchestrator level, before the web UI exposes repo groups.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use agentic_tui::orchestrator::{self, RepoRun, RunConfig};
use agentic_tui::plan::parse_plan;
use shared::StageEvent;
use tokio::sync::mpsc;

static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("base.txt"), "base\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
}

/// A fake `claude` that writes and commits a file named after its cwd's basename,
/// then prints one stream-json result line. Each epic runs in its own worktree,
/// so committing there and merging leaves the file on the integration branch.
fn install_fake_claude(bin_dir: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = bin_dir.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         echo hi > from_epic.txt\n\
         git add -A\n\
         git commit -m 'epic work' >/dev/null 2>&1\n\
         echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.1}'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
}

#[tokio::test]
async fn a_two_repo_plan_merges_each_epic_into_its_own_integration_branch() {
    let _guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("multi-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo_a = base.join("greentic");
    let repo_b = base.join("billing");
    let bin_dir = base.join("bin");
    init_repo(&repo_a);
    init_repo(&repo_b);
    install_fake_claude(&bin_dir);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    let plan = parse_plan(
        r#"{"epics":[
            {"id":"epic-a","title":"A","repo":"greentic","verify":"true","depends_on":[]},
            {"id":"epic-b","title":"B","repo":"billing","verify":"true","depends_on":[]}
        ]}"#,
    )
    .unwrap();

    let mut repos = HashMap::new();
    repos.insert(
        "greentic".to_string(),
        RepoRun {
            path: repo_a.clone(),
            base_ref: "HEAD".to_string(),
            integration_branch: "agentic-integration".to_string(),
        },
    );
    repos.insert(
        "billing".to_string(),
        RepoRun {
            path: repo_b.clone(),
            base_ref: "HEAD".to_string(),
            integration_branch: "agentic-integration".to_string(),
        },
    );

    let config = RunConfig {
        repos,
        goal: "spread work".to_string(),
        default_verify: "true".to_string(),
        initial_cost: 0.0,
        language: shared::Language::English,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();
    let driver = tokio::spawn(async move { orchestrator::run(&plan, config, tx).await });
    // Drain events so the channel does not fill; wait for the driver to finish.
    while rx.recv().await.is_some() {}
    driver.await.unwrap().unwrap();

    std::env::set_var("PATH", original_path);

    let a_file = repo_a.join(".agentic-worktrees/.integration/from_epic.txt");
    let b_file = repo_b.join(".agentic-worktrees/.integration/from_epic.txt");
    assert!(
        a_file.exists(),
        "greentic integration branch must have the epic's file"
    );
    assert!(
        b_file.exists(),
        "billing integration branch must have the epic's file"
    );

    let _ = std::fs::remove_dir_all(&base);
}
