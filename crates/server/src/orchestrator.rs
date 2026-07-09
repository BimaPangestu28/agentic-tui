//! Epic scheduler. The `Scheduler` is a pure state machine: it decides which
//! epics may run now (dependencies satisfied, under the parallel cap) and
//! records outcomes, cascading skips to dependents of failed epics. The async
//! driver that actually spawns sessions is added in a later task.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

use crate::config;
use crate::engine::{self, StageSpec};
use crate::plan::{Epic, Plan};
use crate::worktree::{self, MergeResult};
use shared::StageEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpicState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

pub struct Scheduler {
    order: Vec<String>,
    deps: HashMap<String, Vec<String>>,
    states: HashMap<String, EpicState>,
    max_parallel: usize,
}

impl Scheduler {
    pub fn new(plan: &Plan, max_parallel: usize) -> Self {
        let mut deps = HashMap::new();
        let mut states = HashMap::new();
        let mut order = Vec::new();
        for epic in &plan.epics {
            order.push(epic.id.clone());
            deps.insert(epic.id.clone(), epic.depends_on.clone());
            states.insert(epic.id.clone(), EpicState::Pending);
        }
        Self {
            order,
            deps,
            states,
            max_parallel,
        }
    }

    // Test-only introspection helper: production code drives the scheduler
    // through next_ready/mark_*/snapshot and never queries a single state.
    #[cfg(test)]
    pub fn state(&self, id: &str) -> Option<EpicState> {
        self.states.get(id).copied()
    }

    pub fn running_count(&self) -> usize {
        self.states
            .values()
            .filter(|s| **s == EpicState::Running)
            .count()
    }

    /// Ids that may start now: Pending, all deps Succeeded, in plan order,
    /// limited to the free parallel slots.
    pub fn next_ready(&self) -> Vec<String> {
        let free_slots = self.max_parallel.saturating_sub(self.running_count());
        if free_slots == 0 {
            return Vec::new();
        }
        let mut ready = Vec::new();
        for id in &self.order {
            if self.states[id] != EpicState::Pending {
                continue;
            }
            let deps_ready = self.deps[id]
                .iter()
                .all(|dep| self.states.get(dep) == Some(&EpicState::Succeeded));
            if deps_ready {
                ready.push(id.clone());
                if ready.len() == free_slots {
                    break;
                }
            }
        }
        ready
    }

    pub fn mark_running(&mut self, id: &str) {
        self.set(id, EpicState::Running);
    }

    pub fn mark_succeeded(&mut self, id: &str) {
        self.set(id, EpicState::Succeeded);
    }

    /// Mark an epic failed, then skip every Pending epic that (transitively)
    /// depends on a failed or skipped epic.
    pub fn mark_failed(&mut self, id: &str) {
        self.set(id, EpicState::Failed);
        self.cascade_skips();
    }

    pub fn is_done(&self) -> bool {
        self.states
            .values()
            .all(|s| *s != EpicState::Pending && *s != EpicState::Running)
    }

    /// A copy of every epic id and its current state.
    pub fn snapshot(&self) -> Vec<(String, EpicState)> {
        self.order
            .iter()
            .map(|id| (id.clone(), self.states[id]))
            .collect()
    }

    fn set(&mut self, id: &str, state: EpicState) {
        if let Some(slot) = self.states.get_mut(id) {
            *slot = state;
        }
    }

    fn cascade_skips(&mut self) {
        loop {
            let mut changed = false;
            let to_skip: Vec<String> = self
                .order
                .iter()
                .filter(|id| self.states[*id] == EpicState::Pending)
                .filter(|id| {
                    self.deps[*id].iter().any(|dep| {
                        matches!(
                            self.states.get(dep),
                            Some(EpicState::Failed) | Some(EpicState::Skipped)
                        )
                    })
                })
                .cloned()
                .collect();
            for id in to_skip {
                self.set(&id, EpicState::Skipped);
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }
}

/// One repository a run targets: where it lives, the ref its epics branch
/// from, and the branch their work merges into.
#[derive(Clone)]
pub struct RepoRun {
    pub path: PathBuf,
    pub base_ref: String,
    pub integration_branch: String,
}

pub struct RunConfig {
    pub repos: HashMap<String, RepoRun>,
    pub goal: String,
    pub default_verify: String,
    pub initial_cost: f64,
}

/// The ref an epic's worktree branches from: its repo's integration branch
/// when a dependency lives in the SAME repo (so it inherits merged work),
/// otherwise its repo's base ref. Cross-repo deps do not change the base.
fn epic_base_ref(epic: &Epic, repo_by_id: &HashMap<String, String>, rc: &RepoRun) -> String {
    let has_same_repo_dep = epic
        .depends_on
        .iter()
        .any(|dep| repo_by_id.get(dep) == Some(&epic.repo));
    if has_same_repo_dep {
        rc.integration_branch.clone()
    } else {
        rc.base_ref.clone()
    }
}

/// Run `verify_cmd` inside a worktree. Returns true on exit code 0.
async fn run_verify(worktree_path: &std::path::Path, verify_cmd: &str) -> bool {
    let status = Command::new("sh")
        .arg("-c")
        .arg(verify_cmd)
        .current_dir(worktree_path)
        .kill_on_drop(true)
        .status()
        .await;
    matches!(status, Ok(s) if s.success())
}

/// Run one epic: create worktree, run the session, then verify. On failure,
/// retry once. Accumulates session cost into `spent`. Returns Ok(Some(worktree))
/// if it passed (ready to merge), Ok(None) if it failed after retry.
async fn run_epic(
    epic: &Epic,
    rc: &RepoRun,
    base_ref: &str,
    verify_cmd: &str,
    goal: &str,
    spent: &Arc<Mutex<f64>>,
    tx: &UnboundedSender<StageEvent>,
) -> anyhow::Result<Option<worktree::EpicWorktree>> {
    for attempt in 0..2 {
        // The base ref is resolved by the caller: a dependency-free epic (or
        // one whose deps live in another repo) branches from its repo's base
        // ref; an epic with a same-repo dependency branches from the
        // integration branch, which already holds its merged deps.
        let wt = worktree::create(&rc.path, &epic.id, base_ref).await?;
        let prompt = crate::config::epic_prompt(goal, epic, verify_cmd);
        let spec = StageSpec {
            tag: &epic.id,
            cwd: &wt.path,
            model: crate::config::MODEL_EPIC,
            tools: crate::config::EPIC_TOOLS,
            max_turns: crate::config::EPIC_MAX_TURNS,
            prompt: &prompt,
        };
        let outcome = engine::run_stage(&spec, tx).await?;
        {
            let mut total = spent.lock().await;
            *total += outcome.cost;
            let _ = tx.send(StageEvent::Cost { total: *total });
        }
        let _ = tx.send(StageEvent::EpicVerifying {
            id: epic.id.clone(),
        });
        if outcome.ok && run_verify(&wt.path, verify_cmd).await {
            let _ = tx.send(StageEvent::EpicSucceeded {
                id: epic.id.clone(),
                cost: outcome.cost,
            });
            return Ok(Some(wt));
        }
        let _ = worktree::remove(&rc.path, &wt).await;
        if attempt == 0 {
            let _ = tx.send(StageEvent::StageLog {
                tag: epic.id.clone(),
                line: "verify failed, retrying once".to_string(),
            });
        }
    }
    Ok(None)
}

/// Drive the whole Implement + Integrate flow. Schedules epics respecting
/// dependencies and the parallel cap, verifies each, and merges passing epics
/// into the integration branch in the order they finish. Each stage is bounded
/// by its turn cap (`--max-turns`); there is no cost budget.
pub async fn run(
    plan: &Plan,
    config: RunConfig,
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let epics_by_id: HashMap<String, Epic> = plan
        .epics
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let repo_by_id: HashMap<String, String> = plan
        .epics
        .iter()
        .map(|e| (e.id.clone(), e.repo.clone()))
        .collect();
    let repo_by_id = Arc::new(repo_by_id);
    let scheduler = Arc::new(Mutex::new(Scheduler::new(plan, config::MAX_PARALLEL_EPICS)));
    let config = Arc::new(config);
    let spent = Arc::new(Mutex::new(config.initial_cost));
    // One merge lock per repo. Merges into the same repo's integration branch
    // must not race, but merges into different repos may proceed in parallel.
    let merge_locks: HashMap<String, Arc<Mutex<()>>> = config
        .repos
        .keys()
        .map(|name| (name.clone(), Arc::new(Mutex::new(()))))
        .collect();
    let merge_locks = Arc::new(merge_locks);
    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    loop {
        let ready = {
            let sched = scheduler.lock().await;
            if sched.is_done() {
                break;
            }
            sched.next_ready()
        };

        if ready.is_empty() {
            if let Some(handle) = handles.pop() {
                let _ = handle.await;
            } else {
                break;
            }
            continue;
        }

        for id in ready {
            {
                let mut sched = scheduler.lock().await;
                sched.mark_running(&id);
            }
            let epic = epics_by_id[&id].clone();
            let _ = tx.send(StageEvent::EpicStarted {
                id: epic.id.clone(),
                title: epic.title.clone(),
                repo: epic.repo.clone(),
            });
            let scheduler = scheduler.clone();
            let config = config.clone();
            let repo_by_id = repo_by_id.clone();
            let merge_locks = merge_locks.clone();
            let spent = spent.clone();
            let tx = tx.clone();
            handles.push(tokio::spawn(async move {
                // Resolve the epic's repo. Validation should guarantee it
                // exists; fail the epic defensively if it does not.
                let Some(rc) = config.repos.get(&epic.repo) else {
                    let _ = tx.send(StageEvent::EpicFailed {
                        id: epic.id.clone(),
                        reason: format!("epic names unknown repo {}", epic.repo),
                    });
                    let mut sched = scheduler.lock().await;
                    sched.mark_failed(&epic.id);
                    for (eid, state) in sched.snapshot() {
                        if state == EpicState::Skipped {
                            let _ = tx.send(StageEvent::EpicSkipped { id: eid });
                        }
                    }
                    return;
                };
                let base = epic_base_ref(&epic, &repo_by_id, rc);
                let verify = epic
                    .verify
                    .clone()
                    .unwrap_or_else(|| config.default_verify.clone());
                match run_epic(&epic, rc, &base, &verify, &config.goal, &spent, &tx).await {
                    Ok(Some(wt)) => {
                        let merge_lock = merge_locks[&epic.repo].clone();
                        let merged = {
                            let _guard = merge_lock.lock().await;
                            worktree::merge_into(
                                &rc.path,
                                &wt.branch,
                                &rc.integration_branch,
                                &rc.base_ref,
                            )
                            .await
                        };
                        match merged {
                            Ok(MergeResult::Merged) => {
                                let _ = tx.send(StageEvent::EpicMerged {
                                    id: epic.id.clone(),
                                });
                                {
                                    let mut sched = scheduler.lock().await;
                                    sched.mark_succeeded(&epic.id);
                                }
                                // Remove the worktree only once its work is safely merged.
                                let _ = worktree::remove(&rc.path, &wt).await;
                            }
                            Ok(MergeResult::Conflict) => {
                                let _ = tx.send(StageEvent::EpicConflict {
                                    id: epic.id.clone(),
                                });
                                let mut sched = scheduler.lock().await;
                                sched.mark_failed(&epic.id);
                                // Keep the worktree and branch agentic/<id> for manual merge.
                            }
                            Err(e) => {
                                let _ = tx.send(StageEvent::EpicFailed {
                                    id: epic.id.clone(),
                                    reason: e.to_string(),
                                });
                                let mut sched = scheduler.lock().await;
                                sched.mark_failed(&epic.id);
                                // Keep the worktree and branch for diagnosis.
                            }
                        }
                    }
                    Ok(None) => {
                        let _ = tx.send(StageEvent::EpicFailed {
                            id: epic.id.clone(),
                            reason: "verify failed after retry".to_string(),
                        });
                        let mut sched = scheduler.lock().await;
                        sched.mark_failed(&epic.id);
                    }
                    Err(e) => {
                        let _ = tx.send(StageEvent::EpicFailed {
                            id: epic.id.clone(),
                            reason: e.to_string(),
                        });
                        let mut sched = scheduler.lock().await;
                        sched.mark_failed(&epic.id);
                    }
                }
                let sched = scheduler.lock().await;
                for (eid, state) in sched.snapshot() {
                    if state == EpicState::Skipped {
                        let _ = tx.send(StageEvent::EpicSkipped { id: eid });
                    }
                }
            }));
        }
    }

    for handle in handles {
        let _ = handle.await;
    }
    let _ = tx.send(StageEvent::Done);
    Ok(())
}

/// Re-run one epic that ended blocked (verify failed, merge conflict, or an
/// error) and merge it, reusing the same worktree, session, verify, and merge
/// path a normal epic takes. Emits the same lifecycle events (`EpicStarted`
/// through `EpicMerged`/`EpicConflict`/`EpicFailed`) so the UI updates the
/// card in place. `initial_cost` seeds the accumulated cost so the `Cost`
/// events this retry emits stay monotonic with the finished run's total.
///
/// Unlike `run`, this touches a single epic with no scheduler and no merge
/// lock, since nothing else is running. It does not emit `Done`; the caller
/// detects completion when the event channel closes.
pub async fn retry_epic(
    plan: &Plan,
    epic_id: &str,
    repos: &HashMap<String, RepoRun>,
    goal: &str,
    default_verify: &str,
    initial_cost: f64,
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let epic = plan
        .epics
        .iter()
        .find(|candidate| candidate.id == epic_id)
        .ok_or_else(|| anyhow::anyhow!("epic {epic_id} is not in the plan"))?;
    let repo_by_id: HashMap<String, String> = plan
        .epics
        .iter()
        .map(|e| (e.id.clone(), e.repo.clone()))
        .collect();
    let Some(rc) = repos.get(&epic.repo) else {
        let _ = tx.send(StageEvent::EpicFailed {
            id: epic.id.clone(),
            reason: format!("epic names unknown repo {}", epic.repo),
        });
        return Ok(());
    };

    let base = epic_base_ref(epic, &repo_by_id, rc);
    let verify = epic
        .verify
        .clone()
        .unwrap_or_else(|| default_verify.to_string());
    let spent = Arc::new(Mutex::new(initial_cost));

    let _ = tx.send(StageEvent::EpicStarted {
        id: epic.id.clone(),
        title: epic.title.clone(),
        repo: epic.repo.clone(),
    });

    match run_epic(epic, rc, &base, &verify, goal, &spent, &tx).await {
        Ok(Some(wt)) => {
            let merged =
                worktree::merge_into(&rc.path, &wt.branch, &rc.integration_branch, &rc.base_ref)
                    .await;
            match merged {
                Ok(MergeResult::Merged) => {
                    let _ = tx.send(StageEvent::EpicMerged {
                        id: epic.id.clone(),
                    });
                    let _ = worktree::remove(&rc.path, &wt).await;
                }
                Ok(MergeResult::Conflict) => {
                    let _ = tx.send(StageEvent::EpicConflict {
                        id: epic.id.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(StageEvent::EpicFailed {
                        id: epic.id.clone(),
                        reason: e.to_string(),
                    });
                }
            }
        }
        Ok(None) => {
            let _ = tx.send(StageEvent::EpicFailed {
                id: epic.id.clone(),
                reason: "verify failed after retry".to_string(),
            });
        }
        Err(e) => {
            let _ = tx.send(StageEvent::EpicFailed {
                id: epic.id.clone(),
                reason: e.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan;

    fn scheduler(json: &str, max_parallel: usize) -> Scheduler {
        let plan = parse_plan(json).unwrap();
        Scheduler::new(&plan, max_parallel)
    }

    fn diamond() -> &'static str {
        // a -> b, a -> c, (b,c) -> d
        r#"{"epics":[
            {"id":"a","title":"a"},
            {"id":"b","title":"b","depends_on":["a"]},
            {"id":"c","title":"c","depends_on":["a"]},
            {"id":"d","title":"d","depends_on":["b","c"]}
        ]}"#
    }

    #[test]
    fn only_dependency_free_epics_are_ready_at_first() {
        let sched = scheduler(diamond(), 3);
        assert_eq!(sched.next_ready(), vec!["a".to_string()]);
    }

    #[test]
    fn dependents_become_ready_after_their_dependency_succeeds() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        let mut ready = sched.next_ready();
        ready.sort();
        assert_eq!(ready, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn the_parallel_cap_limits_how_many_are_ready() {
        let mut sched = scheduler(diamond(), 1);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        assert_eq!(sched.next_ready().len(), 1);
    }

    #[test]
    fn running_epics_consume_parallel_slots() {
        let mut sched = scheduler(diamond(), 2);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        sched.mark_running("b");
        // cap 2, one running (b), so one more slot -> exactly one ready (c).
        assert_eq!(sched.next_ready(), vec!["c".to_string()]);
    }

    #[test]
    fn a_failed_epic_skips_its_transitive_dependents() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_failed("a");
        assert_eq!(sched.state("b"), Some(EpicState::Skipped));
        assert_eq!(sched.state("c"), Some(EpicState::Skipped));
        assert_eq!(sched.state("d"), Some(EpicState::Skipped));
        assert!(sched.is_done());
    }

    #[test]
    fn independent_epics_survive_a_failure() {
        let mut sched = scheduler(
            r#"{"epics":[
                {"id":"a","title":"a"},
                {"id":"b","title":"b","depends_on":["a"]},
                {"id":"x","title":"x"}
            ]}"#,
            3,
        );
        sched.mark_running("a");
        sched.mark_failed("a");
        assert_eq!(sched.state("b"), Some(EpicState::Skipped));
        assert_eq!(sched.state("x"), Some(EpicState::Pending));
        assert_eq!(sched.next_ready(), vec!["x".to_string()]);
    }

    #[test]
    fn is_done_only_when_no_epic_is_pending_or_running() {
        let mut sched = scheduler(diamond(), 3);
        assert!(!sched.is_done());
        for id in ["a", "b", "c", "d"] {
            sched.mark_running(id);
            sched.mark_succeeded(id);
        }
        assert!(sched.is_done());
    }

    #[test]
    fn base_ref_uses_integration_only_for_a_same_repo_dep() {
        let rc = RepoRun {
            path: std::path::PathBuf::from("/tmp/x"),
            base_ref: "main".to_string(),
            integration_branch: "agentic-integration".to_string(),
        };
        let mut repo_by_id = HashMap::new();
        repo_by_id.insert("a".to_string(), "greentic".to_string());
        repo_by_id.insert("b".to_string(), "greentic".to_string());
        repo_by_id.insert("c".to_string(), "billing".to_string());

        // same-repo dependency -> integration
        let same = Epic {
            id: "b".into(),
            title: "B".into(),
            repo: "greentic".into(),
            verify: None,
            depends_on: vec!["a".into()],
            acceptance: vec![],
            tasks: vec![],
        };
        assert_eq!(
            epic_base_ref(&same, &repo_by_id, &rc),
            "agentic-integration"
        );

        // cross-repo dependency only -> base
        let cross = Epic {
            id: "c".into(),
            title: "C".into(),
            repo: "billing".into(),
            verify: None,
            depends_on: vec!["a".into()],
            acceptance: vec![],
            tasks: vec![],
        };
        assert_eq!(epic_base_ref(&cross, &repo_by_id, &rc), "main");

        // no dependency -> base
        let free = Epic {
            id: "a".into(),
            title: "A".into(),
            repo: "greentic".into(),
            verify: None,
            depends_on: vec![],
            acceptance: vec![],
            tasks: vec![],
        };
        assert_eq!(epic_base_ref(&free, &repo_by_id, &rc), "main");
    }

    #[test]
    fn snapshot_reports_every_epic_state() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_failed("a");
        let snap = sched.snapshot();
        assert_eq!(snap.len(), 4);
        assert!(snap
            .iter()
            .any(|(id, s)| id == "a" && *s == EpicState::Failed));
    }
}
