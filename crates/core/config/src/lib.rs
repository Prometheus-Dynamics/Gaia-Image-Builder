mod compile;
mod dynamic_inputs;
mod env;
mod interpolate;
mod load;
mod merge;
mod overrides;
mod raw;
mod raw_assembly;

pub use compile::compile_config;

use dynamic_inputs::resolve_dynamic_inputs;
use env::resolve_environment;
use interpolate::interpolate_config;
use load::{discover_build_root, load_build_config};
use merge::merge_config;
use overrides::{apply_cli_overrides, apply_selected_preset, collect_selected_inputs};

use gaia_spec::ResolvedBuildSpec;
use std::fmt;
use std::path::{Path, PathBuf};

pub fn resolve_config(build: &str) -> ResolvedBuildSpec {
    resolve_config_with_options(build, &ResolveOptions::default())
}

pub fn resolve_config_with_options(build: &str, options: &ResolveOptions) -> ResolvedBuildSpec {
    try_resolve_config_with_options(build, options).unwrap_or_else(|error| panic!("{error}"))
}

pub fn try_resolve_config(build: &str) -> Result<ResolvedBuildSpec, ConfigError> {
    try_resolve_config_with_options(build, &ResolveOptions::default())
}

pub fn try_resolve_config_with_options(
    build: &str,
    options: &ResolveOptions,
) -> Result<ResolvedBuildSpec, ConfigError> {
    let span = tracing::info_span!(
        "resolve_config",
        build,
        preset = ?options.preset,
        env_files = options.env_files.len(),
        env_overrides = options.env_overrides.len(),
        explicit_overrides = options.explicit_overrides.len(),
    );
    let _guard = span.enter();
    tracing::debug!(build, preset = ?options.preset, "resolving build config");
    let raw = load_build_config(build)?;
    tracing::debug!(build, "loaded build config");
    let selected = apply_preset_selection(raw, build, options);
    let merged = merge_config(selected);
    let selected = apply_preset_selection(merged, build, options);
    let preset_applied = apply_selected_preset(selected)?;
    let overridden = apply_cli_overrides(preset_applied, options)?;
    let with_dynamic_inputs = resolve_dynamic_inputs(overridden)?;
    let env = resolve_environment(&with_dynamic_inputs)?;
    tracing::debug!(
        build,
        env_files = options.env_files.len(),
        "resolved config environment"
    );
    let interpolated = interpolate_config(with_dynamic_inputs, &env);
    let normalized = normalize_paths(interpolated)?;
    let spec = compile_config(normalized);
    tracing::debug!(
        build,
        build_id = spec.identity.id.as_str(),
        "compiled resolved build spec"
    );
    Ok(spec)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    pub preset: Option<String>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    CurrentDir {
        message: String,
    },
    ConfigNotFound {
        build: String,
        searched: Vec<String>,
    },
    ConfigPath {
        path: String,
        message: String,
    },
    ConfigRead {
        path: String,
        message: String,
    },
    ConfigParse {
        path: String,
        message: String,
    },
    ConfigShape {
        path: String,
        message: String,
    },
    ConfigImportCycle {
        cycle: Vec<String>,
    },
    EnvFileRead {
        path: String,
        message: String,
    },
    MissingPreset {
        preset: String,
    },
    InvalidOverrideValue {
        key: String,
        value: String,
        expected: &'static str,
    },
}

impl ConfigError {
    pub(crate) fn current_dir(error: impl fmt::Display) -> Self {
        Self::CurrentDir {
            message: error.to_string(),
        }
    }

    pub(crate) fn config_path(path: &Path, error: impl fmt::Display) -> Self {
        Self::ConfigPath {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn config_read(path: &Path, error: impl fmt::Display) -> Self {
        Self::ConfigRead {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn config_parse(path: &Path, error: impl fmt::Display) -> Self {
        Self::ConfigParse {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn config_shape(path: &Path, message: impl Into<String>) -> Self {
        Self::ConfigShape {
            path: path.display().to_string(),
            message: message.into(),
        }
    }

    pub(crate) fn env_file_read(path: &Path, error: impl fmt::Display) -> Self {
        Self::EnvFileRead {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    fn invalid_override_value(
        key: impl Into<String>,
        value: impl Into<String>,
        expected: &'static str,
    ) -> Self {
        Self::InvalidOverrideValue {
            key: key.into(),
            value: value.into(),
            expected,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDir { message } => {
                write!(
                    formatter,
                    "current working directory must be readable: {message}"
                )
            }
            Self::ConfigNotFound { build, searched } => write!(
                formatter,
                "failed to locate build config for '{build}', looked in: {}",
                searched.join(", ")
            ),
            Self::ConfigPath { path, message } => {
                write!(formatter, "failed to canonicalize '{path}': {message}")
            }
            Self::ConfigRead { path, message } => {
                write!(formatter, "failed to read build config '{path}': {message}")
            }
            Self::ConfigParse { path, message } => {
                write!(
                    formatter,
                    "failed to parse build config '{path}': {message}"
                )
            }
            Self::ConfigShape { path, message } => {
                write!(
                    formatter,
                    "failed to parse build config '{path}': {message}"
                )
            }
            Self::ConfigImportCycle { cycle } => {
                write!(
                    formatter,
                    "config import cycle detected: {}",
                    cycle.join(" -> ")
                )
            }
            Self::EnvFileRead { path, message } => {
                write!(formatter, "failed to read env file '{path}': {message}")
            }
            Self::MissingPreset { preset } => write!(
                formatter,
                "selected preset '{preset}' was not defined in the resolved config"
            ),
            Self::InvalidOverrideValue {
                key,
                value,
                expected,
            } => write!(
                formatter,
                "invalid override value for '{key}': '{value}' (expected {expected})"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

fn apply_preset_selection(
    mut raw: raw::RawBuildConfig,
    requested_build: &str,
    options: &ResolveOptions,
) -> raw::RawBuildConfig {
    raw.requested_build = Some(requested_build.to_string());
    raw.env_overrides = options.env_overrides.clone();
    raw.explicit_overrides = options.explicit_overrides.clone();
    if let Some(preset) = &options.preset {
        raw.preset = Some(preset.clone());
    }
    raw.selected_inputs = collect_selected_inputs(&raw);
    raw
}

fn normalize_paths(mut raw: raw::RawBuildConfig) -> Result<raw::RawBuildConfig, ConfigError> {
    let build_root = match config_workspace_root(&raw) {
        Some(root) => root,
        None => discover_build_root()?,
    };
    let workspace_root = absolutize(&build_root, &raw.workspace.root_dir);
    raw.workspace.root_dir = workspace_root.display().to_string();
    raw.workspace.build_dir = absolutize(&workspace_root, &raw.workspace.build_dir)
        .display()
        .to_string();
    raw.workspace.out_dir = absolutize(&workspace_root, &raw.workspace.out_dir)
        .display()
        .to_string();
    raw.workspace.named_paths = raw
        .workspace
        .named_paths
        .into_iter()
        .map(|mut entry| {
            if entry.alias.trim().is_empty() || entry.path.trim().is_empty() {
                return Err(ConfigError::ConfigShape {
                    path: raw
                        .source_path
                        .as_deref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    message: "workspace.named_paths entries must use table/object form with non-empty alias/path fields".to_string(),
                });
            }
            entry.path = absolutize(&workspace_root, &entry.path).display().to_string();
            Ok(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(collect_dir) = raw.image.output.collect_dir.clone() {
        raw.image.output.collect_dir = Some(
            absolutize(&workspace_root, &collect_dir)
                .display()
                .to_string(),
        );
    }
    match &mut raw.image.definition {
        raw::RawImageDefinition::Buildroot {
            external_tree: Some(external_tree),
            ..
        } => {
            *external_tree = absolutize(&workspace_root, external_tree)
                .display()
                .to_string();
        }
        raw::RawImageDefinition::StartingPoint {
            source,
            source_path,
            rootfs_path,
            ..
        } => {
            if source.is_none() || rootfs_path.trim().starts_with('/') {
                *rootfs_path = absolutize(&workspace_root, rootfs_path)
                    .display()
                    .to_string();
            }
            if source.is_none()
                && let Some(value) = source_path
            {
                *value = absolutize(&workspace_root, value).display().to_string();
            }
        }
        _ => {}
    }
    Ok(raw)
}

fn config_workspace_root(raw: &raw::RawBuildConfig) -> Option<PathBuf> {
    let source_path = raw.source_path.as_deref()?;
    for ancestor in source_path.ancestors().skip(1) {
        if ancestor.join("Cargo.toml").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn absolutize(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    let joined = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        normalized.push(component.as_os_str());
    }
    normalized
}
