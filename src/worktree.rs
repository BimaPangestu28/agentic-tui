//! Per-epic git worktree lifecycle: create an isolated worktree and branch for
//! an epic, remove it, and merge a passing epic branch into the integration
//! branch. Merge conflicts are reported, never auto-resolved.

use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct EpicWorktree {
    pub id: String,
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

/// Create a worktree and branch for an epic, based off the repo main branch.
pub async fn create(repo: &Path, epic_id: &str) -> anyhow::Result<EpicWorktree> {
    let branch = format!("agentic/{epic_id}");
    let path = worktrees_root(repo).join(epic_id);
    let path_str = path.to_string_lossy().to_string();
    let _ = run_git(repo, &["worktree", "remove", "--force", &path_str]).await;
    let _ = run_git(repo, &["branch", "-D", &branch]).await;
    run_git_checked(repo, &["worktree", "add", "-b", &branch, &path_str, "main"]).await?;
    Ok(EpicWorktree { id: epic_id.to_string(), path, branch })
}

/// Remove an epic worktree and delete its branch.
pub async fn remove(repo: &Path, worktree: &EpicWorktree) -> anyhow::Result<()> {
    let path_str = worktree.path.to_string_lossy().to_string();
    let _ = run_git(repo, &["worktree", "remove", "--force", &path_str]).await;
    let _ = run_git(repo, &["branch", "-D", &worktree.branch]).await;
    Ok(())
}

/// Merge an epic branch into the integration branch, creating it from HEAD on
/// first use. Returns Conflict (and aborts the merge) if it does not apply cleanly.
pub async fn merge_into(
    repo: &Path,
    branch: &str,
    integration_branch: &str,
) -> anyhow::Result<MergeResult> {
    let exists = run_git(repo, &["rev-parse", "--verify", integration_branch])
        .await?
        .status
        .success();
    if !exists {
        run_git_checked(repo, &["branch", integration_branch, "HEAD"]).await?;
    }
    run_git_checked(repo, &["checkout", integration_branch]).await?;
    let merge = run_git(repo, &["merge", "--no-edit", branch]).await?;
    if merge.status.success() {
        Ok(MergeResult::Merged)
    } else {
        let _ = run_git(repo, &["merge", "--abort"]).await;
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
        tokio::fs::write(dir.join("base.txt"), "base\n").await.unwrap();
        git(dir, &["add", "-A"]).await;
        git(dir, &["commit", "-m", "base"]).await;
    }

    #[tokio::test]
    async fn create_and_merge_a_clean_epic() {
        let tmp = std::env::temp_dir().join(format!("wt-clean-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt = create(&tmp, "epic-1").await.unwrap();
        tokio::fs::write(wt.path.join("feature.txt"), "hi\n").await.unwrap();
        git(&wt.path, &["add", "-A"]).await;
        git(&wt.path, &["commit", "-m", "epic-1 work"]).await;

        let result = merge_into(&tmp, &wt.branch, "integration").await.unwrap();
        assert_eq!(result, MergeResult::Merged);
        assert!(tmp.join("feature.txt").exists());

        remove(&tmp, &wt).await.unwrap();
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn a_conflicting_epic_is_reported_not_resolved() {
        let tmp = std::env::temp_dir().join(format!("wt-conflict-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt1 = create(&tmp, "epic-1").await.unwrap();
        tokio::fs::write(wt1.path.join("base.txt"), "from epic-1\n").await.unwrap();
        git(&wt1.path, &["commit", "-am", "epic-1"]).await;
        assert_eq!(merge_into(&tmp, &wt1.branch, "integration").await.unwrap(), MergeResult::Merged);

        let wt2 = create(&tmp, "epic-2").await.unwrap();
        tokio::fs::write(wt2.path.join("base.txt"), "from epic-2\n").await.unwrap();
        git(&wt2.path, &["commit", "-am", "epic-2"]).await;
        assert_eq!(merge_into(&tmp, &wt2.branch, "integration").await.unwrap(), MergeResult::Conflict);

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
}
