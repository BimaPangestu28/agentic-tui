//! Library surface for the agentic-tui server: the multi-stage orchestrator,
//! the embedded web UI backend, and the run manager that drives a pipeline
//! run from an HTTP request.
//!
//! The `agentic-tui` binary (`src/main.rs`) is a thin TUI/CLI shell over this
//! crate, and `crates/server/tests/` integration tests exercise it directly.

pub mod app;
pub mod config;
pub mod engine;
pub mod event;
pub mod http;
pub mod orchestrator;
pub mod plan;
pub mod refine;
pub mod run;
pub mod ui;
pub mod workspace;
pub mod worktree;

use tokio::sync::mpsc;

use event::AppEvent;

/// Resolve a setting by precedence: the CLI flag, then the workspace config,
/// then the built-in default.
pub fn resolve_setting(flag: Option<&str>, configured: Option<&str>, default: &str) -> String {
    flag.or(configured).unwrap_or(default).to_string()
}

/// Plan the goal, then run the orchestrator.
pub async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    base_ref: &str,
    integration: &str,
    refine_cost: f64,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
    // Count refine spending toward the run total before planning starts.
    if refine_cost > 0.0 {
        let _ = tx.send(AppEvent::Stage(shared::StageEvent::Cost {
            total: refine_cost,
        }));
    }
    let plan_path = repo.join(".agentic-plan.json");
    let plan_path_str = plan_path.to_string_lossy().to_string();
    let prompt = config::plan_prompt(goal, &plan_path_str);
    let spec = engine::StageSpec {
        tag: "plan",
        cwd: repo,
        model: config::MODEL_PLAN,
        tools: config::PLAN_TOOLS,
        max_turns: config::PLAN_MAX_TURNS,
        budget_usd: config::EPIC_BUDGET_USD,
        prompt: &prompt,
    };
    let outcome = engine::run_stage(&spec, tx).await?;
    let _ = tx.send(AppEvent::Stage(shared::StageEvent::Cost {
        total: refine_cost + outcome.cost,
    }));

    let plan_text = std::fs::read_to_string(&plan_path)
        .map_err(|e| anyhow::anyhow!("plan.json was not written: {e}"))?;
    let parsed = plan::parse_plan(&plan_text)?;
    parsed.validate()?;
    let epic_metas: Vec<event::EpicMeta> = parsed
        .epics
        .iter()
        .map(|epic| event::EpicMeta {
            id: epic.id.clone(),
            title: epic.title.clone(),
            depends_on: epic.depends_on.clone(),
        })
        .collect();
    let _ = tx.send(AppEvent::Stage(shared::StageEvent::PlanReady {
        epics: epic_metas,
    }));

    let run_config = orchestrator::RunConfig {
        repo: repo.to_path_buf(),
        goal: goal.to_string(),
        verify_cmd: verify_cmd.to_string(),
        integration_branch: integration.to_string(),
        base_ref: base_ref.to_string(),
        budget_usd: config::GLOBAL_BUDGET_USD,
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
