use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub root_dir: String,
    pub build_dir: String,
    pub out_dir: String,
    pub clean_policy: CleanPolicy,
    pub named_paths: Vec<WorkspaceNamedPathSpec>,
}

impl Default for WorkspaceSpec {
    fn default() -> Self {
        Self {
            root_dir: ".".into(),
            build_dir: "build".into(),
            out_dir: "out".into(),
            clean_policy: CleanPolicy::None,
            named_paths: Vec::new(),
        }
    }
}

impl WorkspaceSpec {
    pub fn root_path(&self) -> &Path {
        Path::new(&self.root_dir)
    }

    pub fn build_path(&self) -> &Path {
        Path::new(&self.build_dir)
    }

    pub fn out_path(&self) -> &Path {
        Path::new(&self.out_dir)
    }

    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf, WorkspacePathError> {
        resolve_workspace_path(self, raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceNamedPathSpec {
    pub alias: String,
    pub path: String,
    pub kind: WorkspacePathKindSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanPolicy {
    None,
    Build,
    Out,
    All,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspacePathKindSpec {
    #[default]
    Host,
    Logical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspacePathError {
    EmptyPath,
    EmptyAlias { raw: String },
    UnknownAlias { alias: String, raw: String },
    AbsoluteAliasSuffix { alias: String, raw: String },
    ParentTraversal { raw: String },
}

impl std::fmt::Display for WorkspacePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("workspace path cannot be empty"),
            Self::EmptyAlias { raw } => {
                write!(formatter, "workspace path '{raw}' uses an empty alias")
            }
            Self::UnknownAlias { alias, raw } => {
                write!(formatter, "unknown workspace alias '@{alias}' in '{raw}'")
            }
            Self::AbsoluteAliasSuffix { alias, raw } => write!(
                formatter,
                "workspace path '{raw}' uses an absolute suffix for alias '@{alias}'"
            ),
            Self::ParentTraversal { raw } => {
                write!(
                    formatter,
                    "workspace path '{raw}' escapes its workspace root"
                )
            }
        }
    }
}

impl std::error::Error for WorkspacePathError {}

pub fn resolve_workspace_path(
    workspace: &WorkspaceSpec,
    raw: &str,
) -> Result<PathBuf, WorkspacePathError> {
    if raw.trim().is_empty() {
        return Err(WorkspacePathError::EmptyPath);
    }

    let root = normalize_path(Path::new(&workspace.root_dir))?;
    if let Some(rest) = raw.strip_prefix('@') {
        let (alias, suffix) = rest.split_once('/').unwrap_or((rest, ""));
        if alias.is_empty() {
            return Err(WorkspacePathError::EmptyAlias {
                raw: raw.to_string(),
            });
        }
        let named = workspace
            .named_paths
            .iter()
            .find(|entry| entry.alias == alias)
            .ok_or_else(|| WorkspacePathError::UnknownAlias {
                alias: alias.to_string(),
                raw: raw.to_string(),
            })?;
        let base = resolve_child(&root, &named.path, raw)?;
        if suffix.is_empty() {
            return Ok(base);
        }
        let suffix_path = Path::new(suffix);
        if suffix_path.is_absolute() {
            return Err(WorkspacePathError::AbsoluteAliasSuffix {
                alias: alias.to_string(),
                raw: raw.to_string(),
            });
        }
        return resolve_under(&base, suffix, raw);
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        resolve_under(&root, raw, raw)
    }
}

fn resolve_child(root: &Path, raw: &str, display_raw: &str) -> Result<PathBuf, WorkspacePathError> {
    let path = Path::new(raw);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        resolve_under(root, raw, display_raw)
    }
}

fn resolve_under(root: &Path, raw: &str, display_raw: &str) -> Result<PathBuf, WorkspacePathError> {
    let joined = normalize_path(&root.join(raw))?;
    if root == Path::new(".") || joined.starts_with(root) {
        Ok(joined)
    } else {
        Err(WorkspacePathError::ParentTraversal {
            raw: display_raw.to_string(),
        })
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf, WorkspacePathError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(WorkspacePathError::ParentTraversal {
                    raw: path.display().to_string(),
                });
            }
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(WorkspacePathError::ParentTraversal {
                        raw: path.display().to_string(),
                    });
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> WorkspaceSpec {
        WorkspaceSpec {
            root_dir: "/repo".into(),
            named_paths: vec![WorkspaceNamedPathSpec {
                alias: "assets".into(),
                path: "assets".into(),
                kind: WorkspacePathKindSpec::Host,
            }],
            ..WorkspaceSpec::default()
        }
    }

    #[test]
    fn resolves_relative_and_alias_paths_under_workspace() {
        let workspace = workspace();

        assert_eq!(
            resolve_workspace_path(&workspace, "configs/base.toml").unwrap(),
            PathBuf::from("/repo/configs/base.toml")
        );
        assert_eq!(
            resolve_workspace_path(&workspace, "@assets/etc/motd").unwrap(),
            PathBuf::from("/repo/assets/etc/motd")
        );
    }

    #[test]
    fn rejects_paths_that_escape_workspace_roots() {
        let workspace = workspace();

        assert!(matches!(
            resolve_workspace_path(&workspace, "../outside"),
            Err(WorkspacePathError::ParentTraversal { .. })
        ));
        assert!(matches!(
            resolve_workspace_path(&workspace, "@assets/../../outside"),
            Err(WorkspacePathError::ParentTraversal { .. })
        ));
        assert!(matches!(
            resolve_workspace_path(&workspace, "@missing/file"),
            Err(WorkspacePathError::UnknownAlias { .. })
        ));
    }
}
