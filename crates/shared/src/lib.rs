//! Serde-friendly run state and wire events shared between the terminal
//! server and the (future) wasm web client.
//!
//! Everything in this crate is pure: no tokio, no crossterm, no
//! `std::process`. It must compile for `wasm32-unknown-unknown`.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

const LOG_CAP: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Planning,
    Implementing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
    pub repo: String,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicMeta {
    pub id: String,
    pub title: String,
    pub repo: String,
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
    StageLog {
        tag: String,
        line: String,
    },
    StageAssistant {
        tag: String,
        text: String,
    },
    StageTool {
        tag: String,
        name: String,
    },
    // Lifecycle.
    PlanReady {
        epics: Vec<EpicMeta>,
    },
    EpicStarted {
        id: String,
        title: String,
        repo: String,
    },
    EpicVerifying {
        id: String,
    },
    EpicSucceeded {
        id: String,
        cost: f64,
    },
    EpicFailed {
        id: String,
        reason: String,
    },
    EpicSkipped {
        id: String,
    },
    EpicMerged {
        id: String,
    },
    EpicConflict {
        id: String,
    },
    Cost {
        total: f64,
    },
    Fatal {
        reason: String,
    },
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
                        repo: meta.repo,
                        depends_on: meta.depends_on,
                    })
                    .collect();
                self.push_log(format!("plan ready: {} epics", self.epics.len()));
            }
            StageEvent::EpicStarted { id, title, repo } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: title.clone(),
                        status: EpicStatus::Running,
                        cost: 0.0,
                        repo,
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
                        repo: String::new(),
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
            StageEvent::Cost { total } => self.total_cost = total,
            StageEvent::Fatal { reason } => {
                self.phase = Phase::Failed;
                self.error = Some(reason.clone());
                self.push_log(format!("! FATAL: {reason}"));
            }
            StageEvent::Done => {
                if self.phase != Phase::Failed {
                    self.phase = Phase::Done;
                }
            }
        }
    }
}

/// Wire form of a workspace entry, used at the HTTP API boundary so the
/// web UI does not need to depend on the server's native `Workspace` type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDto {
    pub name: String,
    pub path: String,
    pub base: Option<String>,
    pub integration: Option<String>,
}

/// Body of `POST /api/workspaces/scan`: the directory to scan for repos.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub root: String,
}

/// Response of `POST /api/workspaces/scan`: the repos found under `root`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResponse {
    pub repos: Vec<WorkspaceDto>,
}

/// Body of `POST /api/workspaces`: the full workspace list to persist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveRequest {
    pub workspaces: Vec<WorkspaceDto>,
}

/// Body of `POST /api/runs`: everything needed to start a pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunRequest {
    pub workspace: WorkspaceDto,
    pub goal: String,
    pub base: Option<String>,
    pub into: Option<String>,
    pub verify: Option<String>,
    pub refine_cost: f64,
}

/// Response of `POST /api/runs`: the id of the started run, used to subscribe
/// to its `App` snapshots over `GET /api/runs/{id}/events` and to abort it via
/// `POST /api/runs/{id}/abort`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunResponse {
    pub run_id: String,
}

/// Body of `POST /api/refine/questions`: the repo to refine against and the
/// user's original goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineQuestionsRequest {
    pub repo: String,
    pub goal: String,
}

/// Response of `POST /api/refine/questions`: the rewritten goal, at most
/// `REFINE_MAX_QUESTIONS` clarifying questions, and the cost incurred by the
/// pass (billed even when the result could not be parsed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineQuestionsResponse {
    pub refined_goal: String,
    pub questions: Vec<String>,
    pub cost: f64,
}

/// Body of `POST /api/refine/finalize`: the original goal and the
/// question/answer pairs collected from the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineFinalizeRequest {
    pub repo: String,
    pub goal: String,
    pub answers: Vec<(String, String)>,
}

/// Response of `POST /api/refine/finalize`: the final goal and the cost
/// incurred by the pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineFinalizeResponse {
    pub refined_goal: String,
    pub cost: f64,
}

/// A snapshot of one run for the multi-run dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub goal: String,
    pub phase: Phase,
    pub total_cost: f64,
    pub budget: f64,
    pub epics: Vec<EpicView>,
    pub repos: Vec<String>,
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
                    repo: String::new(),
                    depends_on: vec![],
                },
                EpicMeta {
                    id: "b".to_string(),
                    title: "B".to_string(),
                    repo: String::new(),
                    depends_on: vec!["a".to_string()],
                },
            ],
        });
        assert_eq!(app.epics.len(), 2);
        assert!(app.epics.iter().all(|e| e.status == EpicStatus::Pending));
        let b = app.epics.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(b.depends_on, vec!["a".to_string()]);
    }

    #[test]
    fn plan_ready_seeds_repo_on_each_card() {
        let mut app = App::new("goal".to_string(), "ws".to_string(), 10.0);
        app.apply_stage(StageEvent::PlanReady {
            epics: vec![EpicMeta {
                id: "a".to_string(),
                title: "A".to_string(),
                repo: "greentic".to_string(),
                depends_on: vec![],
            }],
        });
        assert_eq!(app.epics[0].repo, "greentic");
    }

    #[test]
    fn epic_started_carries_repo_onto_a_new_card() {
        let mut app = App::new("g".to_string(), "ws".to_string(), 10.0);
        app.apply_stage(StageEvent::EpicStarted {
            id: "z".to_string(),
            title: "Z".to_string(),
            repo: "billing".to_string(),
        });
        let card = app.epics.iter().find(|e| e.id == "z").unwrap();
        assert_eq!(card.repo, "billing");
    }

    #[test]
    fn stage_events_round_trip_through_json() {
        let events = vec![
            StageEvent::Cost { total: 1.5 },
            StageEvent::Fatal {
                reason: "boom".to_string(),
            },
            StageEvent::StageLog {
                tag: "plan".to_string(),
                line: "hi".to_string(),
            },
            StageEvent::Done,
        ];
        for event in events {
            let json = serde_json::to_string(&event).expect("StageEvent must serialize");
            let back: StageEvent =
                serde_json::from_str(&json).expect("StageEvent must deserialize");
            assert_eq!(format!("{event:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn workspace_dto_round_trips_through_json() {
        let dto = WorkspaceDto {
            name: "greentic".to_string(),
            path: "/tmp/greentic".to_string(),
            base: Some("develop".to_string()),
            integration: None,
        };
        let json = serde_json::to_string(&dto).expect("WorkspaceDto must serialize");
        let back: WorkspaceDto =
            serde_json::from_str(&json).expect("WorkspaceDto must deserialize");
        assert_eq!(dto, back);
    }

    #[test]
    fn start_run_request_round_trips_through_json() {
        let request = StartRunRequest {
            workspace: WorkspaceDto {
                name: "greentic".to_string(),
                path: "/tmp/greentic".to_string(),
                base: None,
                integration: None,
            },
            goal: "add a health check".to_string(),
            base: Some("develop".to_string()),
            into: None,
            verify: Some("make verify".to_string()),
            refine_cost: 0.05,
        };
        let json = serde_json::to_string(&request).expect("StartRunRequest must serialize");
        let back: StartRunRequest =
            serde_json::from_str(&json).expect("StartRunRequest must deserialize");
        assert_eq!(back.workspace.name, "greentic");
        assert_eq!(back.goal, "add a health check");
        assert_eq!(back.base.as_deref(), Some("develop"));
        assert_eq!(back.into, None);
        assert_eq!(back.verify.as_deref(), Some("make verify"));
        assert_eq!(back.refine_cost, 0.05);
    }

    #[test]
    fn start_run_response_round_trips_through_json() {
        let response = StartRunResponse {
            run_id: "1".to_string(),
        };
        let json = serde_json::to_string(&response).expect("StartRunResponse must serialize");
        let back: StartRunResponse =
            serde_json::from_str(&json).expect("StartRunResponse must deserialize");
        assert_eq!(back.run_id, "1");
    }

    #[test]
    fn refine_questions_request_and_response_round_trip_through_json() {
        let request = RefineQuestionsRequest {
            repo: "/tmp/greentic".to_string(),
            goal: "add a health check".to_string(),
        };
        let json = serde_json::to_string(&request).expect("RefineQuestionsRequest must serialize");
        let back: RefineQuestionsRequest =
            serde_json::from_str(&json).expect("RefineQuestionsRequest must deserialize");
        assert_eq!(back.repo, "/tmp/greentic");
        assert_eq!(back.goal, "add a health check");

        let response = RefineQuestionsResponse {
            refined_goal: "add a health check endpoint at /healthz".to_string(),
            questions: vec!["Which port?".to_string()],
            cost: 0.03,
        };
        let json =
            serde_json::to_string(&response).expect("RefineQuestionsResponse must serialize");
        let back: RefineQuestionsResponse =
            serde_json::from_str(&json).expect("RefineQuestionsResponse must deserialize");
        assert_eq!(back.refined_goal, response.refined_goal);
        assert_eq!(back.questions, response.questions);
        assert_eq!(back.cost, response.cost);
    }

    #[test]
    fn refine_finalize_request_and_response_round_trip_through_json() {
        let request = RefineFinalizeRequest {
            repo: "/tmp/greentic".to_string(),
            goal: "add a health check".to_string(),
            answers: vec![("Which port?".to_string(), "8080".to_string())],
        };
        let json = serde_json::to_string(&request).expect("RefineFinalizeRequest must serialize");
        let back: RefineFinalizeRequest =
            serde_json::from_str(&json).expect("RefineFinalizeRequest must deserialize");
        assert_eq!(back.repo, "/tmp/greentic");
        assert_eq!(back.answers, request.answers);

        let response = RefineFinalizeResponse {
            refined_goal: "add a health check endpoint on port 8080".to_string(),
            cost: 0.02,
        };
        let json = serde_json::to_string(&response).expect("RefineFinalizeResponse must serialize");
        let back: RefineFinalizeResponse =
            serde_json::from_str(&json).expect("RefineFinalizeResponse must deserialize");
        assert_eq!(back.refined_goal, response.refined_goal);
        assert_eq!(back.cost, response.cost);
    }

    #[test]
    fn scan_response_round_trips_through_json() {
        let response = ScanResponse {
            repos: vec![WorkspaceDto {
                name: "repoA".to_string(),
                path: "/tmp/repoA".to_string(),
                base: None,
                integration: None,
            }],
        };
        let json = serde_json::to_string(&response).expect("ScanResponse must serialize");
        let back: ScanResponse =
            serde_json::from_str(&json).expect("ScanResponse must deserialize");
        assert_eq!(response.repos, back.repos);
    }

    #[test]
    fn run_summary_round_trips_through_json() {
        let summary = RunSummary {
            id: "1".to_string(),
            workspace: "greentic".to_string(),
            path: "/tmp/greentic".to_string(),
            goal: "Add a health check".to_string(),
            phase: Phase::Implementing,
            total_cost: 0.42,
            budget: 10.0,
            epics: Vec::new(),
            repos: vec!["greentic".to_string()],
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        let back: RunSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.workspace, "greentic");
        assert_eq!(back.phase, summary.phase);
        assert_eq!(back.repos, vec!["greentic".to_string()]);
    }
}
