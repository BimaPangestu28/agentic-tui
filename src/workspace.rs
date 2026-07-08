//! Loading and validating workspaces from `~/.config/agentic-tui/workspaces.toml`.
//! A workspace is a single project root that every session runs inside.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
}

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct WorkspacesFile {
    #[serde(default)]
    workspace: Vec<RawWorkspace>,
}

#[derive(Debug, Deserialize)]
struct RawWorkspace {
    name: String,
    path: String,
}

/// Default location of the workspace registry.
pub fn default_config_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".config").join("agentic-tui").join("workspaces.toml")
}

/// Expand a single leading `~` to the home directory. Other paths pass through.
pub fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(rest)
    } else if raw == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
    } else {
        PathBuf::from(raw)
    }
}

/// Parse workspace entries from a TOML string, expanding paths.
fn parse_workspaces_str(text: &str) -> anyhow::Result<Vec<Workspace>> {
    let parsed: WorkspacesFile = toml::from_str(text)?;
    let workspaces = parsed
        .workspace
        .into_iter()
        .map(|raw| Workspace {
            name: raw.name,
            path: expand_tilde(&raw.path),
        })
        .collect();
    Ok(workspaces)
}

/// Load workspaces from a config file on disk.
pub fn load_workspaces(config_path: &Path) -> anyhow::Result<Vec<Workspace>> {
    let text = std::fs::read_to_string(config_path).map_err(|e| {
        anyhow::anyhow!("could not read {}: {e}", config_path.display())
    })?;
    parse_workspaces_str(&text)
}

/// Ensure a workspace points at a real git repository directory.
pub fn validate(workspace: &Workspace) -> anyhow::Result<()> {
    if !workspace.path.is_dir() {
        anyhow::bail!("workspace path is not a directory: {}", workspace.path.display());
    }
    if !workspace.path.join(".git").exists() {
        anyhow::bail!(
            "workspace is not a git repository (no .git): {}",
            workspace.path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_workspace_list() {
        let toml_text = r#"
[[workspace]]
name = "greentic"
path = "/tmp/greentic"

[[workspace]]
name = "portfolio"
path = "/tmp/portfolio"
"#;
        let workspaces = parse_workspaces_str(toml_text).unwrap();
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[0].name, "greentic");
        assert_eq!(workspaces[0].path, PathBuf::from("/tmp/greentic"));
    }

    #[test]
    fn expands_a_leading_tilde_to_the_home_directory() {
        std::env::set_var("HOME", "/home/tester");
        let expanded = expand_tilde("~/Works/greentic");
        assert_eq!(expanded, PathBuf::from("/home/tester/Works/greentic"));
    }

    #[test]
    fn leaves_absolute_paths_untouched() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn rejects_malformed_toml() {
        let result = parse_workspaces_str("this is not = valid = toml");
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_a_path_that_is_not_a_directory() {
        let workspace = Workspace {
            name: "ghost".to_string(),
            path: PathBuf::from("/nonexistent/path/here"),
        };
        assert!(validate(&workspace).is_err());
    }
}
