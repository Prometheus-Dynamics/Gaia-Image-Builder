use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ConfigError;
use crate::raw::RawBuildConfig;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedEnvironment {
    pub vars: BTreeMap<String, String>,
}

impl ResolvedEnvironment {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }
}

pub fn resolve_environment(raw: &RawBuildConfig) -> Result<ResolvedEnvironment, ConfigError> {
    let mut vars = BTreeMap::new();

    let repo_env = discover_repo_env(raw);
    if repo_env.is_file() {
        vars.extend(load_env_file(&repo_env)?);
    }

    let base_dir = config_dir(raw);
    for env_file in &raw.env_files {
        let path = resolve_relative_path(&base_dir, env_file);
        if path.is_file() {
            vars.extend(load_env_file(&path)?);
        }
    }

    vars.extend(raw.env.clone());

    for (key, value) in env::vars() {
        vars.insert(key, value);
    }

    Ok(ResolvedEnvironment { vars })
}

fn discover_repo_env(raw: &RawBuildConfig) -> PathBuf {
    let base_dir = config_dir(raw);
    let mut current = base_dir.as_path();
    loop {
        let env_path = current.join(".env");
        if env_path.is_file() {
            return env_path;
        }
        let Some(parent) = current.parent() else {
            return base_dir.join(".env");
        };
        current = parent;
    }
}

fn config_dir(raw: &RawBuildConfig) -> PathBuf {
    raw.source_path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn resolve_relative_path(base_dir: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        base_dir.join(candidate)
    }
}

fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>, ConfigError> {
    let contents =
        fs::read_to_string(path).map_err(|error| ConfigError::env_file_read(path, error))?;
    let mut vars = BTreeMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        vars.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }

    Ok(vars)
}
