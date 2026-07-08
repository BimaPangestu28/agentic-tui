//! Epic scheduler. The `Scheduler` is a pure state machine: it decides which
//! epics may run now (dependencies satisfied, under the parallel cap) and
//! records outcomes, cascading skips to dependents of failed epics. The async
//! driver that actually spawns sessions is added in a later task.

use std::collections::HashMap;

use crate::plan::Plan;

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
        Self { order, deps, states, max_parallel }
    }

    pub fn state(&self, id: &str) -> Option<EpicState> {
        self.states.get(id).copied()
    }

    pub fn running_count(&self) -> usize {
        self.states.values().filter(|s| **s == EpicState::Running).count()
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
        self.states.values().all(|s| {
            *s != EpicState::Pending && *s != EpicState::Running
        })
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
    fn snapshot_reports_every_epic_state() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_failed("a");
        let snap = sched.snapshot();
        assert_eq!(snap.len(), 4);
        assert!(snap.iter().any(|(id, s)| id == "a" && *s == EpicState::Failed));
    }
}
