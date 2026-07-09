//! Library surface for the agentic-tui server: the multi-stage orchestrator,
//! the embedded web UI backend, and the run manager that drives a pipeline
//! run from an HTTP request.
//!
//! The `agentic-tui` binary (`src/main.rs`) is a thin launcher over this
//! crate that serves the embedded web UI; `crates/server/tests/` integration
//! tests exercise this crate directly.

pub mod app;
pub mod config;
pub mod engine;
pub mod http;
pub mod orchestrator;
pub mod plan;
pub mod refine;
pub mod run;
pub mod workspace;
pub mod worktree;

use tokio::sync::mpsc;

use shared::{EpicMeta, StageEvent};

/// Resolve a setting by precedence: the CLI flag, then the workspace config,
/// then the built-in default.
pub fn resolve_setting(flag: Option<&str>, configured: Option<&str>, default: &str) -> String {
    flag.or(configured).unwrap_or(default).to_string()
}

/// Plan the goal, then run the orchestrator. `repos` maps each repo name to
/// where it lives and how its epics branch and merge; `default_verify` is the
/// verify command an epic uses when it does not specify its own.
pub async fn run_pipeline(
    plan_cwd: &std::path::Path,
    repos: std::collections::HashMap<String, orchestrator::RepoRun>,
    goal: &str,
    default_verify: &str,
    refine_cost: f64,
    tx: &mpsc::UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    // Count refine spending toward the run total before planning starts.
    if refine_cost > 0.0 {
        let _ = tx.send(StageEvent::Cost { total: refine_cost });
    }
    let plan_path = plan_cwd.join(".agentic-plan.json");
    let plan_path_str = plan_path.to_string_lossy().to_string();
    // Sorted so the prompt lists repos deterministically.
    let repo_list: Vec<(String, String)> = {
        let mut v: Vec<_> = repos
            .iter()
            .map(|(name, rc)| (name.clone(), rc.path.to_string_lossy().to_string()))
            .collect();
        v.sort();
        v
    };
    let prompt = config::plan_prompt(goal, &plan_path_str, &repo_list);
    let spec = engine::StageSpec {
        tag: "plan",
        cwd: plan_cwd,
        model: config::MODEL_PLAN,
        tools: config::PLAN_TOOLS,
        max_turns: config::PLAN_MAX_TURNS,
        prompt: &prompt,
    };
    let outcome = engine::run_stage(&spec, tx).await?;
    let _ = tx.send(StageEvent::Cost {
        total: refine_cost + outcome.cost,
    });

    let plan_text = std::fs::read_to_string(&plan_path)
        .map_err(|e| anyhow::anyhow!("plan.json was not written: {e}"))?;
    let mut parsed = plan::parse_plan(&plan_text)?;
    let names: Vec<String> = repos.keys().cloned().collect();
    // A single-repo run may omit the repo tag on each epic; fill it in.
    if names.len() == 1 {
        parsed.fill_missing_repo(&names[0]);
    }
    parsed.validate()?;
    parsed.validate_repos(&names)?;
    let epic_metas: Vec<EpicMeta> = parsed
        .epics
        .iter()
        .map(|epic| EpicMeta {
            id: epic.id.clone(),
            title: epic.title.clone(),
            repo: epic.repo.clone(),
            depends_on: epic.depends_on.clone(),
        })
        .collect();
    let _ = tx.send(StageEvent::PlanReady { epics: epic_metas });

    let run_config = orchestrator::RunConfig {
        repos,
        goal: goal.to_string(),
        default_verify: default_verify.to_string(),
        initial_cost: refine_cost + outcome.cost,
    };
    orchestrator::run(&parsed, run_config, tx.clone()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_setting_prefers_flag_then_config_then_default() {
        assert_eq!(
            resolve_setting(Some("flag"), Some("config"), "default"),
            "flag"
        );
        assert_eq!(resolve_setting(None, Some("config"), "default"), "config");
        assert_eq!(resolve_setting(None, None, "default"), "default");
    }
}
