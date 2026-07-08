//! State rendered by the UI: the run phase, one view per epic, and a log.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::event::AppEvent;

const LOG_CAP: usize = 2000;

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Planning,
    Implementing,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EpicStatus {
    // Used by the kanban board seeding added in a later task.
    Pending,
    Running,
    Verifying,
    Merged,
    Failed,
    Skipped,
    Conflict,
}

// Used by the kanban board renderer added in a later task.
#[derive(Clone, Copy, PartialEq, Debug)]
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

#[derive(Clone)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
    pub depends_on: Vec<String>,
}

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

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::StageLog { tag, line } => self.push_log(format!("[{tag}] {line}")),
            AppEvent::StageAssistant { tag, text } => self.push_log(format!("[{tag}] . {text}")),
            AppEvent::StageTool { tag, name } => self.push_log(format!("[{tag}] tool: {name}")),
            AppEvent::PlanReady { epics } => {
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
            AppEvent::EpicStarted { id, title } => {
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
            AppEvent::EpicVerifying { id } => self.set_status(&id, EpicStatus::Verifying),
            AppEvent::EpicSucceeded { id, cost } => {
                if let Some(epic) = self.epic_mut(&id) {
                    epic.cost = cost;
                }
                self.push_log(format!("epic {id} passed verify"));
            }
            AppEvent::EpicMerged { id } => {
                self.set_status(&id, EpicStatus::Merged);
                self.push_log(format!("epic {id} merged"));
            }
            AppEvent::EpicFailed { id, reason } => {
                self.set_status(&id, EpicStatus::Failed);
                self.push_log(format!("epic {id} failed: {reason}"));
            }
            AppEvent::EpicSkipped { id } => {
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
            AppEvent::EpicConflict { id } => {
                self.set_status(&id, EpicStatus::Conflict);
                self.push_log(format!("epic {id} merge conflict, needs manual merge"));
            }
            AppEvent::Cost(c) => self.total_cost = c,
            AppEvent::Fatal(s) => {
                self.phase = Phase::Failed;
                self.error = Some(s.clone());
                self.push_log(format!("! FATAL: {s}"));
            }
            AppEvent::Done => {
                if self.phase != Phase::Failed {
                    self.phase = Phase::Done;
                }
            }
            AppEvent::Input(_) | AppEvent::Tick => {}
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
        use crate::event::{AppEvent, EpicMeta};
        let mut app = App::new("goal".to_string(), "ws".to_string(), 10.0);
        app.apply(AppEvent::PlanReady {
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
