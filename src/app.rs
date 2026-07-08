//! State rendered by the UI for the single-stage PRD generator.

use std::collections::VecDeque;
use std::time::Instant;

use crate::event::AppEvent;

const LOG_CAP: usize = 1000;

#[derive(Clone, PartialEq)]
pub enum Phase {
    Running,
    Done,
    Failed,
}

pub struct App {
    pub goal: String,
    pub model: String,
    pub out_path: String,
    pub phase: Phase,
    pub log: VecDeque<String>,
    pub total_cost: f64,
    pub budget: f64,
    pub error: Option<String>,
    pub spinner: usize,
    pub started: Instant,
    pub elapsed_secs: u64,
}

impl App {
    pub fn new(goal: String, budget: f64) -> Self {
        Self {
            goal,
            model: String::new(),
            out_path: String::new(),
            phase: Phase::Running,
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
        if self.phase == Phase::Running {
            self.elapsed_secs = self.started.elapsed().as_secs();
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Started { model, out_path } => {
                self.model = model;
                self.out_path = out_path;
                self.push_log("> generating PRD".to_string());
            }
            AppEvent::Log(s) => self.push_log(format!("  {s}")),
            AppEvent::Assistant(s) => self.push_log(format!("  . {s}")),
            AppEvent::ToolUse(name) => self.push_log(format!("  tool: {name}")),
            AppEvent::Cost(c) => self.total_cost = c,
            AppEvent::Finished { cost, ok } => {
                self.total_cost = cost;
                self.phase = if ok { Phase::Done } else { Phase::Failed };
                self.push_log(if ok {
                    format!("PRD done -> {}", self.out_path)
                } else {
                    "PRD ended with an error".to_string()
                });
            }
            AppEvent::Fatal(s) => {
                self.phase = Phase::Failed;
                self.error = Some(s.clone());
                self.push_log(format!("! FATAL: {s}"));
            }
            AppEvent::Input(_) | AppEvent::Tick => {}
        }
    }
}
