//! The plan model. A plan is a list of epics, each with tasks, acceptance
//! criteria, and dependencies on other epics. Parsed from `plan.json` written
//! by the Plan stage.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Epic {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Plan {
    pub epics: Vec<Epic>,
}

use std::collections::{HashMap, HashSet};

/// Parse a plan from JSON text.
pub fn parse_plan(json: &str) -> anyhow::Result<Plan> {
    let plan: Plan = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("invalid plan.json: {e}"))?;
    Ok(plan)
}

impl Plan {
    /// Check ids are unique, dependencies exist, and there is no cycle.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut ids: HashSet<&str> = HashSet::new();
        for epic in &self.epics {
            if !ids.insert(epic.id.as_str()) {
                anyhow::bail!("duplicate epic id: {}", epic.id);
            }
        }
        for epic in &self.epics {
            for dep in &epic.depends_on {
                if !ids.contains(dep.as_str()) {
                    anyhow::bail!("epic {} depends on unknown epic {}", epic.id, dep);
                }
            }
        }
        // topological_order fails if and only if there is a cycle.
        self.topological_order()?;
        Ok(())
    }

    /// Return epic ids in dependency order (each epic after all its deps).
    /// Ties break by the order epics appear in the plan, for determinism.
    pub fn topological_order(&self) -> anyhow::Result<Vec<String>> {
        let mut remaining_deps: HashMap<&str, HashSet<&str>> = HashMap::new();
        for epic in &self.epics {
            remaining_deps.insert(
                epic.id.as_str(),
                epic.depends_on.iter().map(|d| d.as_str()).collect(),
            );
        }

        let mut order: Vec<String> = Vec::new();
        while order.len() < self.epics.len() {
            // Pick the first epic (in plan order) whose deps are all placed.
            let next = self.epics.iter().find(|epic| {
                let already_placed = order.iter().any(|id| id == &epic.id);
                let deps_ready = remaining_deps[epic.id.as_str()].is_empty();
                !already_placed && deps_ready
            });
            match next {
                Some(epic) => {
                    order.push(epic.id.clone());
                    for deps in remaining_deps.values_mut() {
                        deps.remove(epic.id.as_str());
                    }
                }
                None => anyhow::bail!("dependency cycle detected in plan"),
            }
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_json() -> &'static str {
        r#"{
          "epics": [
            {"id": "a", "title": "Base", "depends_on": [], "acceptance": ["x"],
             "tasks": [{"id": "a1", "title": "do a1", "detail": "details"}]},
            {"id": "b", "title": "Build on base", "depends_on": ["a"], "tasks": []}
          ]
        }"#
    }

    #[test]
    fn parses_epics_with_tasks_and_dependencies() {
        let plan = parse_plan(plan_json()).unwrap();
        assert_eq!(plan.epics.len(), 2);
        assert_eq!(plan.epics[0].tasks[0].id, "a1");
        assert_eq!(plan.epics[1].depends_on, vec!["a".to_string()]);
    }

    #[test]
    fn validate_accepts_a_well_formed_plan() {
        let plan = parse_plan(plan_json()).unwrap();
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn validate_rejects_a_dependency_on_an_unknown_epic() {
        let plan = parse_plan(
            r#"{"epics":[{"id":"a","title":"t","depends_on":["ghost"]}]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_epic_ids() {
        let plan = parse_plan(
            r#"{"epics":[{"id":"a","title":"t"},{"id":"a","title":"u"}]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn validate_rejects_a_dependency_cycle() {
        let plan = parse_plan(
            r#"{"epics":[
                {"id":"a","title":"t","depends_on":["b"]},
                {"id":"b","title":"u","depends_on":["a"]}
            ]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn topological_order_places_dependencies_first() {
        let plan = parse_plan(plan_json()).unwrap();
        let order = plan.topological_order().unwrap();
        let pos_a = order.iter().position(|id| id == "a").unwrap();
        let pos_b = order.iter().position(|id| id == "b").unwrap();
        assert!(pos_a < pos_b);
    }
}
