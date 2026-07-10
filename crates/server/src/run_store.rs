//! On-disk store for runs, so the run registry survives a server restart.
//!
//! Each run is one JSON file at `~/.config/agentic-tui/runs/<id>.json`, in the
//! same config directory as the workspace registry. The run manager writes a
//! snapshot on every lifecycle event and reads them all back at startup. The
//! functions take an explicit `dir` so tests can point at a temporary
//! directory instead of the user's real config.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use shared::App;

/// One repository a run targeted, in the order the run listed it. Rebuilds the
/// `orchestrator::RepoRun` map, the ordered repo names, and the repo paths a
/// live run needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedRepo {
    pub name: String,
    pub path: PathBuf,
    pub base_ref: String,
    pub integration_branch: String,
}

/// Everything needed to show a run as history and to resume it: its identity
/// and config, its own copy of the plan JSON, and the last `App` snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRun {
    pub id: String,
    pub workspace: String,
    pub goal: String,
    pub default_verify: String,
    pub plan_cwd: PathBuf,
    pub repos: Vec<PersistedRepo>,
    /// This run's own copy of `.agentic-plan.json`, so resume never depends on
    /// the shared plan file at the workspace root, which a later run overwrites.
    pub plan_json: String,
    pub app: App,
}

/// Default location of the run store: `~/.config/agentic-tui/runs/`.
pub fn runs_dir() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".config").join("agentic-tui").join("runs")
}

/// Persist one run to `<dir>/<id>.json`. Writes to a `.tmp` sibling first and
/// renames, so a crash mid-write never leaves a torn file. Creates `dir` if it
/// does not exist.
pub fn save(dir: &Path, run: &PersistedRun) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(run)?;
    let final_path = dir.join(format!("{}.json", run.id));
    let tmp_path = dir.join(format!("{}.json.tmp", run.id));
    std::fs::write(&tmp_path, json)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| anyhow::anyhow!("could not rename into {}: {e}", final_path.display()))?;
    Ok(())
}

/// Load every run in `dir`, skipping (with a warning) any file that does not
/// parse, so one corrupt file never blocks startup. Ignores `.tmp` leftovers
/// and anything that is not a `.json` file. Returns an empty vec if `dir` does
/// not exist.
pub fn load_all(dir: &Path) -> Vec<PersistedRun> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut runs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<PersistedRun>(&text) {
                Ok(run) => runs.push(run),
                Err(e) => eprintln!("warning: skipping unreadable run {}: {e}", path.display()),
            },
            Err(e) => eprintln!("warning: could not read run {}: {e}", path.display()),
        }
    }
    runs
}

/// Remove a run's file. A missing file is not an error.
pub fn delete(dir: &Path, id: &str) -> anyhow::Result<()> {
    let path = dir.join(format!("{id}.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("could not delete {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{EpicStatus, EpicView, Phase};

    fn sample_run(id: &str) -> PersistedRun {
        let mut app = App::new("add a health check".to_string(), "greentic".to_string());
        app.phase = Phase::Implementing;
        app.total_cost = 0.42;
        app.epics = vec![EpicView {
            id: "epic-1".to_string(),
            title: "First".to_string(),
            status: EpicStatus::Merged,
            cost: 0.2,
            repo: "greentic".to_string(),
            depends_on: vec![],
            reason: None,
        }];
        PersistedRun {
            id: id.to_string(),
            workspace: "greentic".to_string(),
            goal: "add a health check".to_string(),
            default_verify: "make verify".to_string(),
            plan_cwd: PathBuf::from("/tmp/greentic"),
            repos: vec![PersistedRepo {
                name: "greentic".to_string(),
                path: PathBuf::from("/tmp/greentic"),
                base_ref: "main".to_string(),
                integration_branch: "agentic-integration".to_string(),
            }],
            plan_json: r#"{"epics":[]}"#.to_string(),
            app,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("run-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_round_trips_a_run() {
        let dir = temp_dir("round-trip");
        let run = sample_run("1");
        save(&dir, &run).unwrap();
        let loaded = load_all(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "1");
        assert_eq!(loaded[0].workspace, "greentic");
        assert_eq!(loaded[0].app.total_cost, 0.42);
        assert_eq!(loaded[0].repos, run.repos);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_skips_a_corrupt_file() {
        let dir = temp_dir("corrupt");
        save(&dir, &sample_run("1")).unwrap();
        std::fs::write(dir.join("2.json"), "{ not valid json").unwrap();
        let loaded = load_all(&dir);
        assert_eq!(loaded.len(), 1, "the good run survives a bad neighbor");
        assert_eq!(loaded[0].id, "1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = temp_dir("no-tmp");
        save(&dir, &sample_run("1")).unwrap();
        assert!(dir.join("1.json").exists());
        assert!(!dir.join("1.json.tmp").exists(), "the tmp file is renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_a_run_and_ignores_a_missing_one() {
        let dir = temp_dir("delete");
        save(&dir, &sample_run("1")).unwrap();
        delete(&dir, "1").unwrap();
        assert!(load_all(&dir).is_empty());
        delete(&dir, "1").expect("deleting a missing run is not an error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_on_a_missing_dir_is_empty() {
        let dir = temp_dir("missing");
        assert!(load_all(&dir).is_empty());
    }
}
