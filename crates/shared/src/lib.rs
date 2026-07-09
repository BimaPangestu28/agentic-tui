//! Serde-friendly run state and wire events shared between the terminal
//! server and the (future) wasm web client.
//!
//! Everything in this crate is pure: no tokio, no crossterm, no
//! `std::process`. It must compile for `wasm32-unknown-unknown`.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use serde::{Deserialize, Serialize};

const LOG_CAP: usize = 2000;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Planning,
    Implementing,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EpicStatus {
    // Seeded when the plan is ready; shown in the kanban Todo column.
    Pending,
    Running,
    Verifying,
    Merged,
    Failed,
    Skipped,
    Conflict,
}

// The kanban board column an epic belongs to.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum KanbanColumn {
    Todo,
    InProgress,
    Review,
    Done,
    Blocked,
}

/// Map an epic status to its kanban column.
pub fn kanban_column(status: EpicStatus) -> KanbanColumn {
    match status {
        EpicStatus::Pending => KanbanColumn::Todo,
        EpicStatus::Running => KanbanColumn::InProgress,
        EpicStatus::Verifying => KanbanColumn::Review,
        EpicStatus::Merged => KanbanColumn::Done,
        EpicStatus::Failed | EpicStatus::Skipped | EpicStatus::Conflict => KanbanColumn::Blocked,
    }
}

/// True when any dependency has not yet merged, so a pending epic is on hold.
pub fn is_on_hold(depends_on: &[String], status_by_id: &HashMap<String, EpicStatus>) -> bool {
    depends_on
        .iter()
        .any(|dep| status_by_id.get(dep) != Some(&EpicStatus::Merged))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicMeta {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
}

/// A pipeline event the server applies to `App` and forwards to the browser.
/// This is the terminal-independent subset of the old `AppEvent`: everything
/// except the two variants that only make sense on the terminal (raw key
/// input and the redraw tick), which stay in the server's own `AppEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageEvent {
    // Streaming from a session. `tag` is "plan" or an epic id.
    StageLog { tag: String, line: String },
    StageAssistant { tag: String, text: String },
    StageTool { tag: String, name: String },
    // Lifecycle.
    PlanReady { epics: Vec<EpicMeta> },
    EpicStarted { id: String, title: String },
    EpicVerifying { id: String },
    EpicSucceeded { id: String, cost: f64 },
    EpicFailed { id: String, reason: String },
    EpicSkipped { id: String },
    EpicMerged { id: String },
    EpicConflict { id: String },
    Cost(f64),
    Fatal(String),
    Done,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct App {
    pub goal: String,
    pub workspace: String,
    pub phase: Phase,
    pub epics: Vec<EpicView>,
    pub log: VecDeque<String>,
    pub total_cost: f64,
    pub budget: f64,
    pub error: Option<String>,
    pub spinner: usize,
    // Wall-clock anchor used only to derive `elapsed_secs` in `tick`. Not
    // meaningful across a wire round trip, so it is skipped by serde and
    // reset to "now" on deserialize.
    #[serde(skip, default = "Instant::now")]
    pub started: Instant,
    pub elapsed_secs: u64,
}

impl App {
    pub fn new(goal: String, workspace: String, budget: f64) -> Self {
        Self {
            goal,
            workspace,
            phase: Phase::Planning,
            epics: Vec::new(),
            log: VecDeque::new(),
            total_cost: 0.0,
            budget,
            error: None,
            spinner: 0,
            started: Instant::now(),
            elapsed_secs: 0,
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        if self.phase == Phase::Planning || self.phase == Phase::Implementing {
            self.elapsed_secs = self.started.elapsed().as_secs();
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    fn epic_mut(&mut self, id: &str) -> Option<&mut EpicView> {
        self.epics.iter_mut().find(|e| e.id == id)
    }

    fn set_status(&mut self, id: &str, status: EpicStatus) {
        if let Some(epic) = self.epic_mut(id) {
            epic.status = status;
        }
    }

    /// Apply a wire event to the run state. This is exactly the terminal-
    /// independent logic the old `App::apply` performed for these variants;
    /// the server's `Input`/`Tick` variants are handled by the server itself.
    pub fn apply_stage(&mut self, ev: StageEvent) {
        match ev {
            StageEvent::StageLog { tag, line } => self.push_log(format!("[{tag}] {line}")),
            StageEvent::StageAssistant { tag, text } => self.push_log(format!("[{tag}] . {text}")),
            StageEvent::StageTool { tag, name } => self.push_log(format!("[{tag}] tool: {name}")),
            StageEvent::PlanReady { epics } => {
                self.phase = Phase::Implementing;
                self.epics = epics
                    .into_iter()
                    .map(|meta| EpicView {
                        id: meta.id,
                        title: meta.title,
                        status: EpicStatus::Pending,
                        cost: 0.0,
                        depends_on: meta.depends_on,
                    })
                    .collect();
                self.push_log(format!("plan ready: {} epics", self.epics.len()));
            }
            StageEvent::EpicStarted { id, title } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: title.clone(),
                        status: EpicStatus::Running,
                        cost: 0.0,
                        depends_on: Vec::new(),
                    });
                } else {
                    self.set_status(&id, EpicStatus::Running);
                }
                self.push_log(format!("epic {id} started: {title}"));
            }
            StageEvent::EpicVerifying { id } => self.set_status(&id, EpicStatus::Verifying),
            StageEvent::EpicSucceeded { id, cost } => {
                if let Some(epic) = self.epic_mut(&id) {
                    epic.cost = cost;
                }
                self.push_log(format!("epic {id} passed verify"));
            }
            StageEvent::EpicMerged { id } => {
                self.set_status(&id, EpicStatus::Merged);
                self.push_log(format!("epic {id} merged"));
            }
            StageEvent::EpicFailed { id, reason } => {
                self.set_status(&id, EpicStatus::Failed);
                self.push_log(format!("epic {id} failed: {reason}"));
            }
            StageEvent::EpicSkipped { id } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: String::new(),
                        status: EpicStatus::Skipped,
                        cost: 0.0,
                        depends_on: Vec::new(),
                    });
                } else {
                    self.set_status(&id, EpicStatus::Skipped);
                }
            }
            StageEvent::EpicConflict { id } => {
                self.set_status(&id, EpicStatus::Conflict);
                self.push_log(format!("epic {id} merge conflict, needs manual merge"));
            }
            StageEvent::Cost(c) => self.total_cost = c,
            StageEvent::Fatal(s) => {
                self.phase = Phase::Failed;
                self.error = Some(s.clone());
                self.push_log(format!("! FATAL: {s}"));
            }
            StageEvent::Done => {
                if self.phase != Phase::Failed {
                    self.phase = Phase::Done;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanban_column_maps_each_status() {
        assert_eq!(kanban_column(EpicStatus::Pending), KanbanColumn::Todo);
        assert_eq!(kanban_column(EpicStatus::Running), KanbanColumn::InProgress);
        assert_eq!(kanban_column(EpicStatus::Verifying), KanbanColumn::Review);
        assert_eq!(kanban_column(EpicStatus::Merged), KanbanColumn::Done);
        assert_eq!(kanban_column(EpicStatus::Failed), KanbanColumn::Blocked);
        assert_eq!(kanban_column(EpicStatus::Skipped), KanbanColumn::Blocked);
        assert_eq!(kanban_column(EpicStatus::Conflict), KanbanColumn::Blocked);
    }

    #[test]
    fn is_on_hold_true_when_a_dependency_is_not_merged() {
        let mut status_by_id = HashMap::new();
        status_by_id.insert("a".to_string(), EpicStatus::Running);
        assert!(is_on_hold(&["a".to_string()], &status_by_id));
    }

    #[test]
    fn is_on_hold_false_when_all_dependencies_merged() {
        let mut status_by_id = HashMap::new();
        status_by_id.insert("a".to_string(), EpicStatus::Merged);
        status_by_id.insert("b".to_string(), EpicStatus::Merged);
        assert!(!is_on_hold(
            &["a".to_string(), "b".to_string()],
            &status_by_id
        ));
    }

    #[test]
    fn is_on_hold_true_for_an_unknown_dependency() {
        let status_by_id = HashMap::new();
        assert!(is_on_hold(&["ghost".to_string()], &status_by_id));
    }

    #[test]
    fn no_dependencies_is_never_on_hold() {
        let status_by_id = HashMap::new();
        assert!(!is_on_hold(&[], &status_by_id));
    }

    #[test]
    fn plan_ready_seeds_a_pending_card_per_epic() {
        let mut app = App::new("goal".to_string(), "ws".to_string(), 10.0);
        app.apply_stage(StageEvent::PlanReady {
            epics: vec![
                EpicMeta {
                    id: "a".to_string(),
                    title: "A".to_string(),
                    depends_on: vec![],
                },
                EpicMeta {
                    id: "b".to_string(),
                    title: "B".to_string(),
                    depends_on: vec!["a".to_string()],
                },
            ],
        });
        assert_eq!(app.epics.len(), 2);
        assert!(app.epics.iter().all(|e| e.status == EpicStatus::Pending));
        let b = app.epics.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(b.depends_on, vec!["a".to_string()]);
    }
}
