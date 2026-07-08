//! Per-epic git worktree lifecycle: create an isolated worktree and branch for
//! an epic, remove it, and merge a passing epic branch into the integration
//! branch. Merge conflicts are reported, never auto-resolved.

use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct EpicWorktree {
    pub path: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeResult {
    Merged,
    Conflict,
}

async fn run_git(repo: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to run git {:?}: {e}", args))?;
    Ok(output)
}

async fn run_git_checked(repo: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = run_git(repo, args).await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Directory where epic worktrees live, next to the repo.
fn worktrees_root(repo: &Path) -> PathBuf {
    repo.join(".agentic-worktrees")
}

/// Create a worktree and branch for an epic, based off `base_ref` (a commit,
/// branch, or "HEAD"). Dependency-free epics use HEAD; dependent epics use the
/// integration branch so they inherit their merged dependencies.
pub async fn create(repo: &Path, epic_id: &str, base_ref: &str) -> anyhow::Result<EpicWorktree> {
    let branch = format!("agentic/{epic_id}");
    let path = worktrees_root(repo).join(epic_id);
    let path_str = path.to_string_lossy().to_string();
    let _ = run_git(repo, &["worktree", "remove", "--force", &path_str]).await;
    let _ = run_git(repo, &["branch", "-D", &branch]).await;
    run_git_checked(
        repo,
        &["worktree", "add", "-b", &branch, &path_str, base_ref],
    )
    .await?;
    Ok(EpicWorktree { path, branch })
}

/// Remove an epic worktree and delete its branch. Idempotent: a path that is
/// already gone is not an error. Branch deletion is best-effort.
pub async fn remove(repo: &Path, worktree: &EpicWorktree) -> anyhow::Result<()> {
    let path_str = worktree.path.to_string_lossy().to_string();
    let output = run_git(repo, &["worktree", "remove", "--force", &path_str]).await?;
    if !output.status.success() && worktree.path.exists() {
        anyhow::bail!(
            "failed to remove worktree {}: {}",
            path_str,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let _ = run_git(repo, &["branch", "-D", &worktree.branch]).await;
    Ok(())
}

/// Remove every epic worktree and the `.agentic-worktrees` scratch directory,
/// then prune git's now-stale worktree registrations. Best-effort and used on
/// abort to leave the workspace tidy. Branches are left untouched, so any epic
/// that already merged survives on the integration branch.
pub async fn cleanup_all(repo: &Path) -> anyhow::Result<()> {
    let root = worktrees_root(repo);
    if root.exists() {
        tokio::fs::remove_dir_all(&root).await.ok();
    }
    // Prune after deletion so git forgets the worktrees whose directories are
    // now gone. A repo with no registered worktrees makes this a no-op.
    let _ = run_git(repo, &["worktree", "prune"]).await;
    Ok(())
}

/// Merge an epic branch into the integration branch without disturbing the main
/// working tree. The integration branch is created from the repo HEAD on first
/// use and checked out in a dedicated worktree; all merges happen there. Returns
/// Conflict (and aborts the merge) if the branch does not apply cleanly.
pub async fn merge_into(
    repo: &Path,
    branch: &str,
    integration_branch: &str,
) -> anyhow::Result<MergeResult> {
    // The epic branch must exist, so a genuine merge failure can only be a
    // content conflict, not a bad ref.
    let branch_ok = run_git(repo, &["rev-parse", "--verify", branch])
        .await?
        .status
        .success();
    if !branch_ok {
        anyhow::bail!("cannot merge unknown branch: {branch}");
    }

    // Create the integration branch from HEAD on first use.
    let integration_exists = run_git(repo, &["rev-parse", "--verify", integration_branch])
        .await?
        .status
        .success();
    if !integration_exists {
        run_git_checked(repo, &["branch", integration_branch, "HEAD"]).await?;
    }

    // Ensure a dedicated worktree holds the integration branch, so merges never
    // touch the main working tree.
    let integration_path = worktrees_root(repo).join(".integration");
    let integration_path_str = integration_path.to_string_lossy().to_string();
    if !integration_path.exists() {
        run_git_checked(
            repo,
            &["worktree", "add", &integration_path_str, integration_branch],
        )
        .await?;
    }

    let merge = run_git(&integration_path, &["merge", "--no-edit", branch]).await?;
    if merge.status.success() {
        Ok(MergeResult::Merged)
    } else {
        let _ = run_git(&integration_path, &["merge", "--abort"]).await;
        Ok(MergeResult::Conflict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    async fn init_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]).await;
        git(dir, &["config", "user.email", "t@t.t"]).await;
        git(dir, &["config", "user.name", "t"]).await;
        tokio::fs::write(dir.join("base.txt"), "base\n")
            .await
            .unwrap();
        git(dir, &["add", "-A"]).await;
        git(dir, &["commit", "-m", "base"]).await;
    }

    #[tokio::test]
    async fn create_and_merge_a_clean_epic() {
        let tmp = std::env::temp_dir().join(format!("wt-clean-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt = create(&tmp, "epic-1", "HEAD").await.unwrap();
        tokio::fs::write(wt.path.join("feature.txt"), "hi\n")
            .await
            .unwrap();
        git(&wt.path, &["add", "-A"]).await;
        git(&wt.path, &["commit", "-m", "epic-1 work"]).await;

        let result = merge_into(&tmp, &wt.branch, "integration").await.unwrap();
        assert_eq!(result, MergeResult::Merged);
        let integration_file = tmp.join(".agentic-worktrees/.integration/feature.txt");
        assert!(
            integration_file.exists(),
            "merged file should be present on the integration branch"
        );

        remove(&tmp, &wt).await.unwrap();
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn a_conflicting_epic_is_reported_not_resolved() {
        let tmp = std::env::temp_dir().join(format!("wt-conflict-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt1 = create(&tmp, "epic-1", "HEAD").await.unwrap();
        tokio::fs::write(wt1.path.join("base.txt"), "from epic-1\n")
            .await
            .unwrap();
        git(&wt1.path, &["commit", "-am", "epic-1"]).await;
        assert_eq!(
            merge_into(&tmp, &wt1.branch, "integration").await.unwrap(),
            MergeResult::Merged
        );

        let wt2 = create(&tmp, "epic-2", "HEAD").await.unwrap();
        tokio::fs::write(wt2.path.join("base.txt"), "from epic-2\n")
            .await
            .unwrap();
        git(&wt2.path, &["commit", "-am", "epic-2"]).await;
        assert_eq!(
            merge_into(&tmp, &wt2.branch, "integration").await.unwrap(),
            MergeResult::Conflict
        );

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn cleanup_all_removes_worktrees_but_keeps_merged_branches() {
        let tmp = std::env::temp_dir().join(format!("wt-cleanup-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        // One epic worktree, and its work merged into the integration branch
        // (which creates the .integration worktree too).
        let wt = create(&tmp, "epic-1", "HEAD").await.unwrap();
        tokio::fs::write(wt.path.join("feature.txt"), "hi\n")
            .await
            .unwrap();
        git(&wt.path, &["add", "-A"]).await;
        git(&wt.path, &["commit", "-m", "epic-1 work"]).await;
        assert_eq!(
            merge_into(&tmp, &wt.branch, "integration").await.unwrap(),
            MergeResult::Merged
        );

        cleanup_all(&tmp).await.unwrap();

        // The scratch directory is gone and git no longer lists any worktree
        // other than the main checkout.
        assert!(
            !tmp.join(".agentic-worktrees").exists(),
            "scratch directory should be removed"
        );
        let listed = Command::new("git")
            .args(["worktree", "list"])
            .current_dir(&tmp)
            .output()
            .await
            .unwrap();
        let listing = String::from_utf8_lossy(&listed.stdout);
        assert!(
            !listing.contains(".agentic-worktrees"),
            "no epic worktrees should remain registered, got:\n{listing}"
        );

        // The merged work still lives on the integration branch.
        let branch = Command::new("git")
            .args(["rev-parse", "--verify", "integration"])
            .current_dir(&tmp)
            .output()
            .await
            .unwrap();
        assert!(
            branch.status.success(),
            "integration branch should survive cleanup"
        );

        // cleanup_all is idempotent: a second run on a tidy repo is a no-op.
        cleanup_all(&tmp).await.unwrap();

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn an_epic_based_on_integration_inherits_merged_work() {
        let tmp = std::env::temp_dir().join(format!("wt-baseref-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        // First epic adds a file and merges into the integration branch.
        let wt1 = create(&tmp, "epic-1", "HEAD").await.unwrap();
        tokio::fs::write(wt1.path.join("from_a.txt"), "a\n")
            .await
            .unwrap();
        git(&wt1.path, &["add", "-A"]).await;
        git(&wt1.path, &["commit", "-m", "epic-1"]).await;
        assert_eq!(
            merge_into(&tmp, &wt1.branch, "integration").await.unwrap(),
            MergeResult::Merged
        );

        // A dependent epic based on the integration branch must see epic-1's file.
        let wt2 = create(&tmp, "epic-2", "integration").await.unwrap();
        assert!(
            wt2.path.join("from_a.txt").exists(),
            "epic based on integration should inherit merged dependency work"
        );

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
}
