//! State rendered by the UI: the run phase, one view per epic, and a log.

use std::collections::VecDeque;
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
    Running,
    Verifying,
    Merged,
    Failed,
    Skipped,
    Conflict,
}

#[derive(Clone)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
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
            AppEvent::PlanReady { epic_count } => {
                self.phase = Phase::Implementing;
                self.push_log(format!("plan ready: {epic_count} epics"));
            }
            AppEvent::EpicStarted { id, title } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: title.clone(),
                        status: EpicStatus::Running,
                        cost: 0.0,
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
