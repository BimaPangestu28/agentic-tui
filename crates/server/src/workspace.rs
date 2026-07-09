//! Loading and validating workspaces from `~/.config/agentic-tui/workspaces.toml`.
//! A workspace is a named group of one or more git repositories that a run
//! targets together. A one-repo group behaves exactly as a single-repo
//! workspace did before this reshape.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One repository inside a workspace group: where it lives, the ref its epics
/// branch from (defaulting later to `HEAD`), and the branch their work merges
/// into (defaulting later to `agentic-integration`).
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub base: Option<String>,
    pub integration: Option<String>,
}

/// A named group of repositories a run targets together.
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub repos: Vec<Repo>,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct WorkspacesFile {
    #[serde(default)]
    workspace: Vec<RawWorkspace>,
}

/// A `[[workspace]]` entry as parsed from TOML. It accepts BOTH shapes: a
/// nested list of `[[workspace.repo]]` blocks (a multi-repo group), or a
/// legacy flat `path`/`base`/`integration` (a one-repo group named after the
/// workspace). Existing flat configs keep working.
#[derive(Debug, Deserialize)]
struct RawWorkspace {
    name: String,
    // Legacy single-repo fields:
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    integration: Option<String>,
    // Nested repo list:
    #[serde(default)]
    repo: Vec<RawRepo>,
}

#[derive(Debug, Deserialize)]
struct RawRepo {
    name: String,
    path: String,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    integration: Option<String>,
}

#[derive(Serialize)]
struct WorkspacesOut {
    workspace: Vec<RawWorkspaceOut>,
}

/// A workspace group always serializes to the nested shape: `name` plus one
/// `[[workspace.repo]]` block per repo (a one-repo group writes a single
/// block).
#[derive(Serialize)]
struct RawWorkspaceOut {
    name: String,
    repo: Vec<RawRepoOut>,
}

#[derive(Serialize)]
struct RawRepoOut {
    name: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    integration: Option<String>,
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

/// Parse workspace entries from a TOML string, expanding paths. Each entry
/// becomes a `Workspace` group: a nested `[[workspace.repo]]` list maps to a
/// multi-repo group; a legacy flat `path` maps to a one-repo group named after
/// the workspace. An entry with neither a repo list nor a flat path is an
/// error.
fn parse_workspaces_str(text: &str) -> anyhow::Result<Vec<Workspace>> {
    let parsed: WorkspacesFile = toml::from_str(text)?;
    parsed
        .workspace
        .into_iter()
        .map(|raw| {
            let repos = if !raw.repo.is_empty() {
                raw.repo
                    .into_iter()
                    .map(|repo| Repo {
                        name: repo.name,
                        path: expand_tilde(&repo.path),
                        base: repo.base,
                        integration: repo.integration,
                    })
                    .collect()
            } else if let Some(path) = raw.path {
                // A legacy flat entry becomes a one-repo group named after the
                // workspace itself.
                vec![Repo {
                    name: raw.name.clone(),
                    path: expand_tilde(&path),
                    base: raw.base,
                    integration: raw.integration,
                }]
            } else {
                anyhow::bail!(
                    "workspace '{}' has neither a path nor a [[workspace.repo]] list",
                    raw.name
                );
            };
            Ok(Workspace {
                name: raw.name,
                repos,
            })
        })
        .collect()
}

/// Load workspaces from a config file on disk.
pub fn load_workspaces(config_path: &Path) -> anyhow::Result<Vec<Workspace>> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", config_path.display()))?;
    parse_workspaces_str(&text)
}

/// Persist `workspaces` to `config_path`, merging with any entries already
/// saved there. Entries are unioned by workspace name and the existing group
/// wins on a name conflict. The parent directory is created if it does not
/// exist.
///
/// The merge starts from an empty list only when `config_path` does not
/// exist yet. If the file exists but cannot be parsed (malformed or
/// unreadable), this function returns an error and writes nothing, so an
/// existing config is never silently overwritten.
pub fn save_workspaces(config_path: &Path, workspaces: &[Workspace]) -> anyhow::Result<()> {
    let mut merged: Vec<Workspace> = match load_workspaces(config_path) {
        Ok(existing) => existing,
        Err(_) if !config_path.exists() => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut seen: HashSet<String> = merged.iter().map(|w| w.name.clone()).collect();
    for workspace in workspaces {
        if seen.insert(workspace.name.clone()) {
            merged.push(workspace.clone());
        }
    }

    let out = WorkspacesOut {
        workspace: merged
            .iter()
            .map(|w| RawWorkspaceOut {
                name: w.name.clone(),
                repo: w
                    .repos
                    .iter()
                    .map(|r| RawRepoOut {
                        name: r.name.clone(),
                        path: r.path.to_string_lossy().to_string(),
                        base: r.base.clone(),
                        integration: r.integration.clone(),
                    })
                    .collect(),
            })
            .collect(),
    };
    let text = toml::to_string(&out)?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, text)?;
    Ok(())
}

/// Ensure a workspace is a non-empty group of git repositories with unique
/// repo names, each repo path being a directory that contains `.git`. The
/// error names the offending repo.
pub fn validate(workspace: &Workspace) -> anyhow::Result<()> {
    if workspace.repos.is_empty() {
        anyhow::bail!("workspace '{}' has no repositories", workspace.name);
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for repo in &workspace.repos {
        if !seen.insert(repo.name.as_str()) {
            anyhow::bail!(
                "workspace '{}' has a duplicate repo name: {}",
                workspace.name,
                repo.name
            );
        }
        if !repo.path.is_dir() {
            anyhow::bail!(
                "repo '{}' path is not a directory: {}",
                repo.name,
                repo.path.display()
            );
        }
        if !repo.path.join(".git").exists() {
            anyhow::bail!(
                "repo '{}' is not a git repository (no .git): {}",
                repo.name,
                repo.path.display()
            );
        }
    }
    Ok(())
}

/// The longest shared directory prefix of every repo path in the group. For a
/// single repo it is that repo's parent directory, so planning runs one level
/// above the repo just as it did before this reshape.
pub fn common_root(workspace: &Workspace) -> PathBuf {
    let mut paths = workspace.repos.iter().map(|r| r.path.as_path());
    let Some(first) = paths.next() else {
        return PathBuf::new();
    };
    if workspace.repos.len() == 1 {
        return first
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| first.to_path_buf());
    }
    // Start from the first path's components and shrink to the longest prefix
    // shared with every other path.
    let mut prefix: Vec<_> = first.components().collect();
    for path in paths {
        let components: Vec<_> = path.components().collect();
        let shared = prefix
            .iter()
            .zip(components.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(shared);
    }
    prefix.iter().collect()
}

/// Maximum directory depth `scan_for_repos` descends from its root.
pub const DEFAULT_SCAN_DEPTH: usize = 6;

/// Upper bound on how many repositories a single scan returns, so a huge tree
/// cannot hang the wizard.
pub const MAX_SCAN_RESULTS: usize = 500;

/// Recursively scan `root` for git repositories, descending at most `max_depth`
/// directory levels. A directory that contains a `.git` entry is a repository
/// and is not descended into. Hidden directories (names starting with `.`) and
/// heavy build directories are pruned. Unreadable directories are skipped
/// silently. Results are sorted by path, capped at `MAX_SCAN_RESULTS`, and named
/// by their directory, disambiguated by parent directory on a name collision.
/// The grouping of these raw repos into a named `Workspace` happens at the
/// save layer, so this returns the flat repo list.
pub fn scan_for_repos(root: &Path, max_depth: usize) -> Vec<Repo> {
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
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => {}
                _ => continue,
            }
            let path = entry.path();
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
    repos_from_paths(repos)
}

/// The final path component as a display name, falling back to the whole path.
fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Turn repo paths into repos, disambiguating any names shared by more than
/// one repo with a `parent/name` label.
fn repos_from_paths(paths: Vec<PathBuf>) -> Vec<Repo> {
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
            Repo {
                name,
                path,
                base: None,
                integration: None,
            }
        })
        .collect()
}

/// Guards tests (here and in `http.rs`) that mutate the process-wide `HOME`
/// env var, since `cargo test` runs tests in parallel within one process and
/// an unguarded mutation would let two such tests race on the same variable.
/// A `tokio::sync::Mutex` is used, not `std::sync::Mutex`, so the async tests
/// in `http.rs` can hold the guard across an `.await` point.
#[cfg(test)]
pub(crate) static HOME_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        assert_eq!(workspaces[0].repos.len(), 1);
        assert_eq!(workspaces[0].repos[0].path, PathBuf::from("/tmp/greentic"));
    }

    #[test]
    fn parses_a_nested_multi_repo_workspace() {
        let toml_text = r#"
[[workspace]]
name = "greentic"

  [[workspace.repo]]
  name = "greentic"
  path = "/tmp/greentic/greentic"
  base = "main"

  [[workspace.repo]]
  name = "billing"
  path = "/tmp/greentic/billing"
"#;
        let ws = parse_workspaces_str(toml_text).unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].repos.len(), 2);
        assert_eq!(ws[0].repos[0].name, "greentic");
        assert_eq!(ws[0].repos[0].base.as_deref(), Some("main"));
        assert_eq!(ws[0].repos[1].name, "billing");
    }

    #[test]
    fn parses_a_legacy_flat_workspace_as_one_repo() {
        let toml_text = r#"
[[workspace]]
name = "greentic"
path = "/tmp/greentic/greentic"
base = "develop"
"#;
        let ws = parse_workspaces_str(toml_text).unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(
            ws[0].repos.len(),
            1,
            "a flat entry becomes a one-repo group"
        );
        assert_eq!(ws[0].repos[0].name, "greentic");
        assert_eq!(ws[0].repos[0].path, PathBuf::from("/tmp/greentic/greentic"));
        assert_eq!(ws[0].repos[0].base.as_deref(), Some("develop"));
    }

    #[test]
    fn validate_rejects_an_empty_repo_list_and_duplicate_names() {
        let empty = Workspace {
            name: "x".into(),
            repos: vec![],
        };
        assert!(validate(&empty).is_err());
    }

    #[test]
    fn common_root_is_the_shared_parent_of_sibling_repos() {
        let ws = Workspace {
            name: "greentic".into(),
            repos: vec![
                Repo {
                    name: "a".into(),
                    path: PathBuf::from("/home/u/greentic/a"),
                    base: None,
                    integration: None,
                },
                Repo {
                    name: "b".into(),
                    path: PathBuf::from("/home/u/greentic/b"),
                    base: None,
                    integration: None,
                },
            ],
        };
        assert_eq!(common_root(&ws), PathBuf::from("/home/u/greentic"));
    }

    #[test]
    fn common_root_of_one_repo_is_its_parent() {
        let ws = Workspace {
            name: "solo".into(),
            repos: vec![Repo {
                name: "solo".into(),
                path: PathBuf::from("/home/u/greentic/solo"),
                base: None,
                integration: None,
            }],
        };
        assert_eq!(common_root(&ws), PathBuf::from("/home/u/greentic"));
    }

    #[test]
    fn expands_a_leading_tilde_to_the_home_directory() {
        let _guard = HOME_ENV_LOCK.blocking_lock();
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
            repos: vec![Repo {
                name: "ghost".to_string(),
                path: PathBuf::from("/nonexistent/path/here"),
                base: None,
                integration: None,
            }],
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

    /// A convenience for tests: a one-repo group named `name` rooted at `path`.
    fn one_repo(name: &str, path: &str) -> Workspace {
        Workspace {
            name: name.to_string(),
            repos: vec![Repo {
                name: name.to_string(),
                path: PathBuf::from(path),
                base: None,
                integration: None,
            }],
        }
    }

    #[test]
    fn save_unions_with_existing_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("nested/workspaces.toml");

        let first = vec![one_repo("a", "/tmp/a"), one_repo("b", "/tmp/b")];
        save_workspaces(&config, &first).unwrap();
        let loaded = load_workspaces(&config).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0], first[0]);

        // A second save with an overlapping name unions by name; the existing
        // group named "b" is kept, and "c" is added.
        let more = vec![
            {
                let mut renamed = one_repo("b", "/tmp/b-different");
                renamed.name = "b".to_string();
                renamed
            },
            one_repo("c", "/tmp/c"),
        ];
        save_workspaces(&config, &more).unwrap();
        let loaded = load_workspaces(&config).unwrap();
        assert_eq!(
            loaded.len(),
            3,
            "union should be a, b, c with no duplicate b"
        );
        let b = loaded.iter().find(|w| w.name == "b").unwrap();
        assert_eq!(
            b.repos[0].path,
            PathBuf::from("/tmp/b"),
            "existing group is kept on a name conflict"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_round_trips_a_nested_multi_repo_group() {
        let dir = std::env::temp_dir().join(format!("save-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("workspaces.toml");

        let group = Workspace {
            name: "greentic".to_string(),
            repos: vec![
                Repo {
                    name: "greentic".to_string(),
                    path: PathBuf::from("/tmp/greentic/greentic"),
                    base: Some("main".to_string()),
                    integration: None,
                },
                Repo {
                    name: "billing".to_string(),
                    path: PathBuf::from("/tmp/greentic/billing"),
                    base: None,
                    integration: None,
                },
            ],
        };
        save_workspaces(&config, std::slice::from_ref(&group)).unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains("[[workspace.repo]]"),
            "a group must serialize to the nested repo shape"
        );

        let loaded = load_workspaces(&config).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], group);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_refuses_to_overwrite_a_malformed_config() {
        let dir = std::env::temp_dir().join(format!("save-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("workspaces.toml");
        let malformed = "this is = not = valid toml";
        std::fs::write(&config, malformed).unwrap();

        let result = save_workspaces(&config, &[one_repo("new", "/tmp/new")]);
        assert!(
            result.is_err(),
            "save_workspaces must not overwrite a config it cannot parse"
        );

        let on_disk = std::fs::read_to_string(&config).unwrap();
        assert_eq!(
            on_disk, malformed,
            "the malformed file must be left untouched"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_skips_symlinked_directories() {
        let base = std::env::temp_dir().join(format!("scan-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("real/.git")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();

        let found = scan_for_repos(&base, DEFAULT_SCAN_DEPTH);
        let paths: std::collections::HashSet<_> = found.iter().map(|w| w.path.clone()).collect();

        assert!(paths.contains(&base.join("real")));
        assert!(
            !paths.contains(&base.join("link")),
            "a symlinked directory must not be traversed"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parses_optional_base_and_integration() {
        let toml_text = r#"
[[workspace]]
name = "a"
path = "/tmp/a"
base = "develop"
integration = "agentic-wip"

[[workspace]]
name = "b"
path = "/tmp/b"
"#;
        let ws = parse_workspaces_str(toml_text).unwrap();
        assert_eq!(ws[0].repos[0].base.as_deref(), Some("develop"));
        assert_eq!(ws[0].repos[0].integration.as_deref(), Some("agentic-wip"));
        assert_eq!(ws[1].repos[0].base, None);
        assert_eq!(ws[1].repos[0].integration, None);
    }

    #[test]
    fn save_round_trips_base_and_integration() {
        let dir = std::env::temp_dir().join(format!("save-branches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("workspaces.toml");
        let list = vec![
            Workspace {
                name: "a".to_string(),
                repos: vec![Repo {
                    name: "a".to_string(),
                    path: PathBuf::from("/tmp/a"),
                    base: Some("develop".to_string()),
                    integration: Some("agentic-wip".to_string()),
                }],
            },
            one_repo("b", "/tmp/b"),
        ];
        save_workspaces(&config, &list).unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            !text.contains("base = \"\""),
            "an unset field must not serialize as an empty key"
        );

        let loaded = load_workspaces(&config).unwrap();
        let a = loaded.iter().find(|w| w.name == "a").unwrap();
        assert_eq!(a.repos[0].base.as_deref(), Some("develop"));
        assert_eq!(a.repos[0].integration.as_deref(), Some("agentic-wip"));
        let b = loaded.iter().find(|w| w.name == "b").unwrap();
        assert_eq!(b.repos[0].base, None);
        assert_eq!(b.repos[0].integration, None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
