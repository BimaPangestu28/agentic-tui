//! `run_resume` skips epics that are seeded as already merged and runs only the
//! rest. Uses a fake `claude` on PATH, mirroring `tests/multi_repo.rs`.

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
async fn run_resume_skips_seeded_epics_and_runs_the_rest() {
    let _guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("run-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("greentic");
    let bin_dir = base.join("bin");
    init_repo(&repo);
    install_fake_claude(&bin_dir);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    // Two independent epics. epic-a is seeded as already merged, so only epic-b
    // should run.
    let plan = parse_plan(
        r#"{"epics":[
            {"id":"epic-a","title":"A","repo":"greentic","verify":"true","depends_on":[]},
            {"id":"epic-b","title":"B","repo":"greentic","verify":"true","depends_on":[]}
        ]}"#,
    )
    .unwrap();

    let mut repos = HashMap::new();
    repos.insert(
        "greentic".to_string(),
        RepoRun {
            path: repo.clone(),
            base_ref: "HEAD".to_string(),
            integration_branch: "agentic-integration".to_string(),
        },
    );
    let config = RunConfig {
        repos,
        goal: "resume".to_string(),
        default_verify: "true".to_string(),
        initial_cost: 0.0,
        language: shared::Language::English,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();
    let driver = tokio::spawn(async move {
        orchestrator::run_resume(&plan, config, &["epic-a".to_string()], tx).await
    });

    let mut started: Vec<String> = Vec::new();
    while let Some(ev) = rx.recv().await {
        if let StageEvent::EpicStarted { id, .. } = ev {
            started.push(id);
        }
    }
    driver.await.unwrap().unwrap();

    std::env::set_var("PATH", original_path);

    assert_eq!(
        started,
        vec!["epic-b".to_string()],
        "only the non-seeded epic runs"
    );

    let _ = std::fs::remove_dir_all(&base);
}
