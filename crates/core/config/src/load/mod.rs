use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ConfigError;
use crate::raw::RawBuildConfig;

pub fn discover_build_root() -> Result<PathBuf, ConfigError> {
    let mut current = env::current_dir().map_err(ConfigError::current_dir)?;
    loop {
        if current.join("Cargo.toml").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return env::current_dir().map_err(ConfigError::current_dir);
        }
    }
}

pub fn load_build_config(build: &str) -> Result<RawBuildConfig, ConfigError> {
    tracing::debug!(build, "resolving build config path");
    let build_path = resolve_build_path(build)?;
    tracing::debug!(path = %build_path.display(), "loading build config");
    let mut loading_stack = Vec::new();
    let config = load_build_config_from_path(&build_path, &mut loading_stack)?;
    tracing::debug!(
        build_name = %config.build_name,
        imports = config.imported_configs.len(),
        has_extends = config.extends_config.is_some(),
        "build config loaded"
    );
    Ok(config)
}

fn load_build_config_from_path(
    path: &Path,
    loading_stack: &mut Vec<PathBuf>,
) -> Result<RawBuildConfig, ConfigError> {
    let canonical_path =
        fs::canonicalize(path).map_err(|error| ConfigError::config_path(path, error))?;
    if loading_stack.contains(&canonical_path) {
        let mut cycle = loading_stack
            .iter()
            .map(|entry| entry.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(canonical_path.display().to_string());
        return Err(ConfigError::ConfigImportCycle { cycle });
    }

    loading_stack.push(canonical_path.clone());
    tracing::trace!(
        path = %canonical_path.display(),
        depth = loading_stack.len(),
        "reading config file"
    );
    let contents = fs::read_to_string(&canonical_path)
        .map_err(|error| ConfigError::config_read(&canonical_path, error))?;
    let value: toml::Value = toml::from_str(&contents)
        .map_err(|error| ConfigError::config_parse(&canonical_path, error))?;
    validate_raw_toml_shape(&canonical_path, &value)?;
    let mut raw: RawBuildConfig = value
        .try_into()
        .map_err(|error| ConfigError::config_parse(&canonical_path, error))?;
    raw.source_path = Some(canonical_path.clone());
    if raw.build_name.trim().is_empty() {
        raw.build_name = infer_build_name(&canonical_path);
    }

    let config_dir = canonical_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if let Some(extends) = &raw.extends {
        let extends_path = resolve_relative_config_path(&config_dir, extends);
        tracing::trace!(
            path = %canonical_path.display(),
            extends = %extends_path.display(),
            "loading extended config"
        );
        raw.extends_config = Some(Box::new(load_build_config_from_path(
            &extends_path,
            loading_stack,
        )?));
    }
    raw.imported_configs = raw
        .imports
        .iter()
        .map(|import| {
            (
                import.clone(),
                resolve_relative_config_path(&config_dir, &import.path),
            )
        })
        .inspect(|import_path| {
            tracing::trace!(
                path = %canonical_path.display(),
                import = %import_path.1.display(),
                "loading imported config"
            );
        })
        .map(|(import, import_path)| {
            load_build_config_from_path(&import_path, loading_stack)
                .map(|config| crate::raw::RawImportedConfig { import, config })
        })
        .collect::<Result<Vec<_>, _>>()?;

    loading_stack.pop();
    Ok(raw)
}

fn validate_raw_toml_shape(path: &Path, value: &toml::Value) -> Result<(), ConfigError> {
    let Some(workspace) = value.get("workspace").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    let Some(named_paths) = workspace.get("named_paths") else {
        return Ok(());
    };
    let Some(entries) = named_paths.as_array() else {
        return Err(ConfigError::config_shape(
            path,
            "workspace.named_paths must be an array of tables",
        ));
    };
    for entry in entries {
        if !entry.is_table() {
            return Err(ConfigError::config_shape(
                path,
                "workspace.named_paths entries must use table/object form with alias/path/kind fields",
            ));
        }
    }
    Ok(())
}

fn resolve_build_path(build: &str) -> Result<PathBuf, ConfigError> {
    let input = PathBuf::from(build);
    if input.is_file() {
        return Ok(input);
    }

    let root = discover_build_root()?;
    let candidates = [
        root.join(build),
        root.join("configs").join(format!("{build}.toml")),
        root.join("configs")
            .join("builds")
            .join(format!("{build}.toml")),
        root.join("examples").join(build).join("build.toml"),
    ];

    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| ConfigError::ConfigNotFound {
            build: build.to_string(),
            searched: candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        })
}

fn resolve_relative_config_path(base_dir: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        base_dir.join(candidate)
    }
}

fn infer_build_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("build")
        .to_string()
}
