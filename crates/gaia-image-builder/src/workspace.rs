use std::collections::BTreeMap;
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

fn default_true() -> bool {
    true
}

fn default_build_dir() -> String {
    "build".into()
}

fn default_out_dir() -> String {
    "out".into()
}

fn default_root_dir() -> String {
    ".".into()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanMode {
    None,
    Build,
    Out,
    All,
}

impl Default for CleanMode {
    fn default() -> Self {
        CleanMode::None
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_root_dir")]
    pub root_dir: String,
    #[serde(default = "default_build_dir")]
    pub build_dir: String,
    #[serde(default = "default_out_dir")]
    pub out_dir: String,
    #[serde(default)]
    pub paths: BTreeMap<String, String>,
    #[serde(default)]
    pub clean: CleanMode,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root_dir: default_root_dir(),
            build_dir: default_build_dir(),
            out_dir: default_out_dir(),
            paths: BTreeMap::new(),
            clean: CleanMode::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    pub build_dir: PathBuf,
    pub out_dir: PathBuf,
    pub named_dirs: BTreeMap<String, PathBuf>,
}

impl WorkspacePaths {
    // Resolve a user-configured path:
    // - `@alias/...` expands from `[workspace.paths.alias]`
    // - absolute paths are used as-is
    // - relative paths are rooted at workspace root
    pub fn resolve_config_path(&self, raw: &str) -> Result<PathBuf> {
        resolve_config_path(self, raw)
    }

    pub fn resolve_under_root(&self, rel: &str) -> Result<PathBuf> {
        resolve_under(&self.root, &self.root, rel)
    }

    pub fn resolve_under_build(&self, rel: &str) -> Result<PathBuf> {
        resolve_under(&self.root, &self.build_dir, rel)
    }

    pub fn resolve_under_out(&self, rel: &str) -> Result<PathBuf> {
        resolve_under(&self.root, &self.out_dir, rel)
    }
}

pub fn load_paths(cfg: &WorkspaceConfig) -> Result<WorkspacePaths> {
    let cwd = std::env::current_dir().map_err(|e| Error::msg(format!("cwd error: {e}")))?;
    let root = resolve_user_path(&cwd, &cfg.root_dir)?;
    let build_dir = resolve_user_dir(&root, &cfg.build_dir)?;
    let out_dir = resolve_user_dir(&root, &cfg.out_dir)?;
    let named_dirs = resolve_named_dirs(&root, &build_dir, &out_dir, &cfg.paths)?;
    Ok(WorkspacePaths {
        root,
        build_dir,
        out_dir,
        named_dirs,
    })
}

pub fn init_dirs(cfg: &WorkspaceConfig) -> Result<WorkspacePaths> {
    let paths = load_paths(cfg)?;

    match cfg.clean {
        CleanMode::None => {}
        CleanMode::Build => safe_remove_dir_all(&paths.root, &paths.build_dir)?,
        CleanMode::Out => safe_remove_dir_all(&paths.root, &paths.out_dir)?,
        CleanMode::All => {
            safe_remove_dir_all(&paths.root, &paths.build_dir)?;
            safe_remove_dir_all(&paths.root, &paths.out_dir)?;
        }
    }

    fs::create_dir_all(&paths.build_dir).map_err(|e| {
        Error::msg(format!(
            "failed to create build_dir {}: {e}",
            paths.build_dir.display()
        ))
    })?;
    fs::create_dir_all(&paths.out_dir).map_err(|e| {
        Error::msg(format!(
            "failed to create out_dir {}: {e}",
            paths.out_dir.display()
        ))
    })?;

    Ok(paths)
}

fn resolve_user_dir(root: &Path, p: &str) -> Result<PathBuf> {
    let p = p.trim();
    if p.is_empty() {
        return Err(Error::msg("empty workspace dir"));
    }
    let rel_pb = Path::new(p);
    if rel_pb
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::msg(format!(
            "invalid workspace dir '{}' (contains '..')",
            p
        )));
    }
    let pb = PathBuf::from(p);
    let joined = if pb.is_absolute() { pb } else { root.join(pb) };
    // Canonicalize parent/root for safety comparisons, but tolerate non-existent directories.
    Ok(joined)
}

fn resolve_user_path(base: &Path, p: &str) -> Result<PathBuf> {
    let p = p.trim();
    if p.is_empty() {
        return Err(Error::msg("empty workspace path"));
    }
    let pb = PathBuf::from(p);
    Ok(if pb.is_absolute() { pb } else { base.join(pb) })
}

fn resolve_named_dirs(
    root: &Path,
    build_dir: &Path,
    out_dir: &Path,
    paths: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::<String, PathBuf>::new();
    out.insert("root".into(), root.to_path_buf());
    out.insert("build".into(), build_dir.to_path_buf());
    out.insert("out".into(), out_dir.to_path_buf());

    for (name, raw) in paths {
        let key = name.trim();
        if key.is_empty() {
            return Err(Error::msg("workspace.paths has an empty key"));
        }
        if !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::msg(format!(
                "workspace.paths key '{}' is invalid (allowed: a-zA-Z0-9_-)",
                key
            )));
        }
        if key == "root" || key == "build" || key == "out" {
            return Err(Error::msg(format!(
                "workspace.paths key '{}' is reserved",
                key
            )));
        }
        let resolved = resolve_user_path(root, raw)?;
        out.insert(key.to_string(), resolved);
    }

    Ok(out)
}

fn safe_remove_dir_all(root: &Path, dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let root_can = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let dir_can = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !dir_can.starts_with(&root_can) {
        return Err(Error::msg(format!(
            "refusing to remove '{}' (outside workspace root '{}')",
            dir_can.display(),
            root_can.display()
        )));
    }
    fs::remove_dir_all(&dir_can)
        .map_err(|e| Error::msg(format!("failed to remove dir {}: {e}", dir_can.display())))?;
    Ok(())
}

fn resolve_config_path(ws: &WorkspacePaths, raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::msg("empty path"));
    }

    if let Some(after_at) = raw.strip_prefix('@') {
        let (alias, rest) = if let Some((a, r)) = after_at.split_once('/') {
            (a.trim(), Some(r))
        } else {
            (after_at.trim(), None)
        };
        if alias.is_empty() {
            return Err(Error::msg(format!("invalid alias path '{}'", raw)));
        }
        let base = ws.named_dirs.get(alias).ok_or_else(|| {
            let known = ws.named_dirs.keys().cloned().collect::<Vec<_>>().join(", ");
            Error::msg(format!(
                "unknown workspace path alias '{}' in '{}' (known: {})",
                alias, raw, known
            ))
        })?;
        return Ok(match rest {
            Some(r) if !r.is_empty() => base.join(r),
            _ => base.to_path_buf(),
        });
    }

    let pb = PathBuf::from(raw);
    Ok(if pb.is_absolute() {
        pb
    } else {
        ws.root.join(pb)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_alias_and_relative_paths() {
        let root = PathBuf::from("/tmp/gaia-root");
        let build_dir = root.join("build");
        let out_dir = root.join("out");
        let mut named_dirs = BTreeMap::<String, PathBuf>::new();
        named_dirs.insert("root".into(), root.clone());
        named_dirs.insert("build".into(), build_dir.clone());
        named_dirs.insert("out".into(), out_dir.clone());
        named_dirs.insert("assets".into(), root.join("assets"));

        let ws = WorkspacePaths {
            root: root.clone(),
            build_dir,
            out_dir,
            named_dirs,
        };

        assert_eq!(
            ws.resolve_config_path("assets/file.txt")
                .expect("relative path"),
            root.join("assets/file.txt")
        );
        assert_eq!(
            ws.resolve_config_path("@assets/services/a.service")
                .expect("alias path"),
            root.join("assets/services/a.service")
        );
    }
}

fn resolve_under(root: &Path, base: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(Error::msg("empty relative path"));
    }
    let rel_pb = Path::new(rel);
    if rel_pb
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::msg(format!(
            "invalid relative path '{}' (contains '..')",
            rel
        )));
    }
    let pb = PathBuf::from(rel);
    let out = if pb.is_absolute() { pb } else { base.join(pb) };
    // If absolute, still enforce it's within root.
    let root_can = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let out_can = out.canonicalize().unwrap_or_else(|_| out.clone());
    if !out_can.starts_with(&root_can) {
        return Err(Error::msg(format!(
            "refusing path '{}' (outside workspace root '{}')",
            out.display(),
            root.display()
        )));
    }
    Ok(out)
}
