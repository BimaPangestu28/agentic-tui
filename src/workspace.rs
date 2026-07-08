//! Loading and validating workspaces from `~/.config/agentic-tui/workspaces.toml`.
//! A workspace is a single project root that every session runs inside.

use std::collections::HashMap;
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
    base.join(".config")
        .join("agentic-tui")
        .join("workspaces.toml")
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
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", config_path.display()))?;
    parse_workspaces_str(&text)
}

/// Ensure a workspace points at a real git repository directory.
pub fn validate(workspace: &Workspace) -> anyhow::Result<()> {
    if !workspace.path.is_dir() {
        anyhow::bail!(
            "workspace path is not a directory: {}",
            workspace.path.display()
        );
    }
    if !workspace.path.join(".git").exists() {
        anyhow::bail!(
            "workspace is not a git repository (no .git): {}",
            workspace.path.display()
        );
    }
    Ok(())
}

/// Maximum directory depth `scan_for_repos` descends from its root.
#[allow(dead_code)]
pub const DEFAULT_SCAN_DEPTH: usize = 6;

/// Upper bound on how many repositories a single scan returns, so a huge tree
/// cannot hang the wizard.
#[allow(dead_code)]
pub const MAX_SCAN_RESULTS: usize = 500;

/// Recursively scan `root` for git repositories, descending at most `max_depth`
/// directory levels. A directory that contains a `.git` entry is a repository
/// and is not descended into. Hidden directories (names starting with `.`) and
/// heavy build directories are pruned. Unreadable directories are skipped
/// silently. Results are sorted by path, capped at `MAX_SCAN_RESULTS`, and named
/// by their directory, disambiguated by parent directory on a name collision.
#[allow(dead_code)]
pub fn scan_for_repos(root: &Path, max_depth: usize) -> Vec<Workspace> {
    const PRUNE: [&str; 4] = ["node_modules", "target", "dist", "build"];
    let mut repos: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if repos.len() >= MAX_SCAN_RESULTS {
            break;
        }
        if dir.join(".git").exists() {
            repos.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || PRUNE.contains(&name.as_ref()) {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    repos.sort();
    repos.truncate(MAX_SCAN_RESULTS);
    workspaces_from_paths(repos)
}

/// The final path component as a display name, falling back to the whole path.
#[allow(dead_code)]
fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Turn repo paths into workspaces, disambiguating any names shared by more than
/// one repo with a `parent/name` label.
#[allow(dead_code)]
fn workspaces_from_paths(paths: Vec<PathBuf>) -> Vec<Workspace> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for path in &paths {
        *counts.entry(base_name(path)).or_insert(0) += 1;
    }
    paths
        .into_iter()
        .map(|path| {
            let base = base_name(&path);
            let name = if counts.get(&base).copied().unwrap_or(0) > 1 {
                match path.parent().and_then(|parent| parent.file_name()) {
                    Some(parent) => format!("{}/{}", parent.to_string_lossy(), base),
                    None => base,
                }
            } else {
                base
            };
            Workspace { name, path }
        })
        .collect()
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

    #[test]
    fn scan_finds_repos_and_prunes_noise() {
        let base = std::env::temp_dir().join(format!("scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("repoA/.git")).unwrap();
        // A repo nested inside another repo must not be found: we do not descend
        // into a directory that is already a repo.
        std::fs::create_dir_all(base.join("repoA/inner/.git")).unwrap();
        std::fs::create_dir_all(base.join("group/repoB/.git")).unwrap();
        // Pruned locations: build noise and hidden directories.
        std::fs::create_dir_all(base.join("node_modules/ghost/.git")).unwrap();
        std::fs::create_dir_all(base.join(".hidden/repoC/.git")).unwrap();

        let found = scan_for_repos(&base, DEFAULT_SCAN_DEPTH);
        let paths: std::collections::HashSet<_> = found.iter().map(|w| w.path.clone()).collect();

        assert!(paths.contains(&base.join("repoA")));
        assert!(paths.contains(&base.join("group/repoB")));
        assert!(
            !paths.contains(&base.join("repoA/inner")),
            "must not descend into a repo"
        );
        assert!(
            !paths.contains(&base.join("node_modules/ghost")),
            "node_modules must be pruned"
        );
        assert!(
            !paths.contains(&base.join(".hidden/repoC")),
            "hidden directories must be pruned"
        );
        assert_eq!(found.len(), 2);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_disambiguates_duplicate_names() {
        let base = std::env::temp_dir().join(format!("scan-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("x/proj/.git")).unwrap();
        std::fs::create_dir_all(base.join("y/proj/.git")).unwrap();

        let found = scan_for_repos(&base, DEFAULT_SCAN_DEPTH);
        let mut names: Vec<_> = found.iter().map(|w| w.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["x/proj".to_string(), "y/proj".to_string()]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
