mod assembly_source_tokens;
mod compile;
mod dynamic_inputs;
mod env;
mod gaia_version;
mod interpolate;
mod load;
pub mod lockfile;
mod merge;
mod overrides;
mod raw;
mod raw_assembly;
mod raw_variant;
mod raw_when;
mod source_tokens;
mod unknown_keys;
mod variants;

pub use compile::compile_config;
pub use gaia_version::GAIA_VERSION;
pub use variants::{
    resolve_variant_specs, resolve_variant_specs_scoped, variant_output_collisions,
};

use dynamic_inputs::resolve_dynamic_inputs;
use env::resolve_environment;
use interpolate::interpolate_config;
use load::{discover_build_root, load_build_config};
use merge::merge_config;
use overrides::{apply_cli_overrides, apply_selected_presets, collect_selected_inputs};

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
    resolve_build(build, options, &VariantResolution::default())
}

/// What a spec is resolved as, when it is one variant of a build: its id and
/// the sources it checks out for itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct VariantResolution {
    pub(crate) id: Option<String>,
    pub(crate) scoped_sources: std::collections::BTreeSet<String>,
}

/// A resolved spec, and the `--set sources.<id>.path` overrides it could not
/// apply because the config it resolved does not declare that source.
pub(crate) struct ResolvedBuild {
    pub(crate) spec: ResolvedBuildSpec,
    pub(crate) undeclared_sources: Vec<(String, String)>,
    pub(crate) declared_sources: Vec<String>,
}

impl ResolvedBuild {
    /// The spec, if no override named an undeclared source.
    pub(crate) fn into_checked_spec(self) -> Result<ResolvedBuildSpec, ConfigError> {
        match self.undeclared_sources.first() {
            Some((key, _)) => Err(undeclared_source_error(
                key,
                "the selected config",
                self.declared_sources,
            )),
            None => Ok(self.spec),
        }
    }
}

pub(crate) fn undeclared_source_error(
    key: &str,
    scope: &str,
    declared: Vec<String>,
) -> ConfigError {
    ConfigError::UndeclaredSourceOverride {
        key: key.to_string(),
        source_id: overrides::source_path_override_id(key)
            .unwrap_or_default()
            .to_string(),
        scope: scope.to_string(),
        declared,
    }
}

pub(crate) fn resolve_build(
    build: &str,
    options: &ResolveOptions,
    variant: &VariantResolution,
) -> Result<ResolvedBuildSpec, ConfigError> {
    resolve_build_checked(build, options, variant)?.into_checked_spec()
}

pub(crate) fn resolve_build_checked(
    build: &str,
    options: &ResolveOptions,
    variant: &VariantResolution,
) -> Result<ResolvedBuild, ConfigError> {
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
    let load::LoadedBuildConfig {
        raw,
        import_sources,
        unused_import_sources,
    } = load_build_config(build, options)?;
    if let Some(name) = raw_when::unknown_when_input(&raw) {
        return Err(ConfigError::ConfigShape {
            path: build.to_string(),
            message: format!(
                "a 'when' condition names input '{name}', which no file of the build declares"
            ),
        });
    }
    tracing::debug!(
        build,
        import_sources = import_sources.len(),
        "loaded build config"
    );
    let unknown_key_warnings = unknown_keys::collect_unknown_key_warnings(&raw);
    let empty_layer_warnings = unknown_keys::collect_empty_layer_warnings(&raw);
    let selected = apply_preset_selection(raw, build, options);
    let merged = merge_config(selected);
    variants::validate_declared(&merged)?;
    let selected = apply_preset_selection(merged, build, options);
    let preset_applied = apply_selected_presets(selected, &options.variant_presets)?;
    let (overridden, undeclared_sources) = apply_cli_overrides(preset_applied, options)?;
    let declared_sources = overrides::declared_source_ids(&overridden);
    let with_dynamic_inputs = resolve_dynamic_inputs(overridden)?;
    let env = resolve_environment(&with_dynamic_inputs)?;
    tracing::debug!(
        build,
        env_files = options.env_files.len(),
        "resolved config environment"
    );
    let interpolated = interpolate_config(with_dynamic_inputs, &env);
    let normalized = normalize_paths(interpolated)?;
    reject_non_rust_build_groups(&normalized)?;
    reject_non_rust_cargo_git_sources(&normalized)?;
    let mut spec = compile_config(normalized);
    if let Some(id) = &variant.id {
        for source in spec
            .sources
            .iter_mut()
            .filter(|source| variant.scoped_sources.contains(source.id.as_str()))
        {
            source.variant_scope = Some(id.clone());
        }
    }
    spec.metadata.config_warnings = unknown_key_warnings;
    spec.metadata.empty_layers = empty_layer_warnings;
    spec.selection.import_sources = import_sources;
    drop_unused_import_sources(&mut spec, &unused_import_sources);
    apply_lockfile(&mut spec);
    source_tokens::substitute_source_tokens(&mut spec);
    tracing::debug!(
        build,
        build_id = spec.identity.id.as_str(),
        "compiled resolved build spec"
    );
    Ok(ResolvedBuild {
        spec,
        undeclared_sources,
        declared_sources,
    })
}

/// Removes sources declared only for importing config into layers this
/// selection does not use (for example a device package source in a
/// multi-target build), so they are neither planned nor fetched. A source
/// that an artifact or the image also builds from is kept.
fn drop_unused_import_sources(spec: &mut ResolvedBuildSpec, unused: &[String]) {
    if unused.is_empty() {
        return;
    }
    let image_source = match &spec.image.definition {
        gaia_spec::ImageDefinition::Buildroot(buildroot) => buildroot.source.clone(),
        gaia_spec::ImageDefinition::StartingPoint(starting_point) => starting_point.source.clone(),
    };
    let built_from = |id: &str| {
        image_source
            .as_ref()
            .is_some_and(|source| source.as_str() == id)
            || spec.artifacts.iter().any(|artifact| {
                artifact
                    .source
                    .as_ref()
                    .is_some_and(|source| source.id.as_str() == id)
            })
    };
    let drop = unused
        .iter()
        .filter(|id| !built_from(id))
        .cloned()
        .collect::<Vec<_>>();
    if drop.is_empty() {
        return;
    }
    tracing::debug!(sources = ?drop, "dropping import-only sources of non-selected layers");
    spec.sources
        .retain(|source| !drop.iter().any(|id| id == source.id.as_str()));
}

/// Pins git sources to the commits recorded in the build's lockfile. An
/// unreadable lockfile is ignored here and reported by validation.
fn apply_lockfile(spec: &mut ResolvedBuildSpec) {
    let Some(path) = lockfile::lockfile_path(spec) else {
        return;
    };
    match lockfile::GitLockfile::load(&path) {
        Ok(Some(lock)) => lockfile::apply_git_lockfile(spec, &lock),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "ignoring unreadable lockfile"),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    pub preset: Option<String>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    /// Presets a variant applies in place of `preset`, in order. Set by the
    /// variant resolution only.
    pub variant_presets: Vec<String>,
    /// The variants the command acts on, by id; empty selects all. An
    /// override of `sources.<id>.path` must be declared by one of them.
    pub selected_variants: Vec<String>,
    /// Resolve import sources that have neither `rev` nor a lockfile entry
    /// to their current commit instead of failing. Used by `gaia lock`.
    pub resolve_unpinned_import_sources: bool,
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
    /// An `imports` or `extends` entry names a file that does not exist.
    ConfigReferenceMissing {
        referenced_by: String,
        field: &'static str,
        reference: String,
        resolved: String,
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
    /// `sources.<id>.path` names a source that no config the build selects
    /// declares. `scope` names what was searched, such as "the selected
    /// config" or "any selected variant".
    UndeclaredSourceOverride {
        key: String,
        source_id: String,
        scope: String,
        declared: Vec<String>,
    },
    /// The build file's `gaia_version` requirement does not match this
    /// binary.
    GaiaVersionUnsupported {
        path: String,
        required: String,
        installed: String,
    },
    /// An `imports` entry with `source = "<id>"` (or an `@source:<id>`
    /// reference) could not be resolved.
    ImportSource {
        source_id: String,
        referenced_by: String,
        message: String,
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
            Self::ConfigReferenceMissing {
                referenced_by,
                field,
                reference,
                resolved,
            } => write!(
                formatter,
                "{field} '{reference}' in '{referenced_by}' does not exist (resolved to '{resolved}')"
            ),
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
            Self::UndeclaredSourceOverride {
                key,
                source_id,
                scope,
                declared,
            } => {
                let declared = if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.join(", ")
                };
                write!(
                    formatter,
                    "override '{key}' names source '{source_id}', which is not declared by {scope} (declared sources: {declared})"
                )
            }
            Self::GaiaVersionUnsupported {
                path,
                required,
                installed,
            } => write!(
                formatter,
                "this build requires gaia {required}, but gaia {installed} is installed \
                 (gaia_version in '{path}'); upgrade with: {}",
                gaia_version::UPGRADE_COMMAND
            ),
            Self::ImportSource {
                source_id,
                referenced_by,
                message,
            } => write!(
                formatter,
                "import source '{source_id}' used by '{referenced_by}': {message}"
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
    // A variant's presets replace the selected preset; the last one is the
    // selected preset. Their `input` overrides select the inputs that
    // imports and `when` conditions see.
    let chain = if options.variant_presets.is_empty() {
        raw.preset.clone().into_iter().collect::<Vec<_>>()
    } else {
        raw.preset = options.variant_presets.last().cloned();
        options.variant_presets.clone()
    };
    for name in chain {
        let Some(preset) = raw.presets.get(&name) else {
            continue;
        };
        let inputs = preset
            .overrides
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix("input.")
                    .or_else(|| key.strip_prefix("inputs."))
                    .map(|input| (input.to_string(), value.clone()))
            })
            .collect::<Vec<_>>();
        for (input, value) in inputs {
            overrides::upsert_pair(&mut raw.selected_inputs, &input, &value);
        }
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
    let source_path = raw
        .source_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("<unknown>"));
    raw.image.output.export_dir = normalize_export_dir(
        raw.image.output.export_dir.take(),
        "image.output.export_dir",
        &source_path,
        &workspace_root,
    )?;
    raw.workspace.export_dir = normalize_export_dir(
        raw.workspace.export_dir.take(),
        "workspace.export_dir",
        &source_path,
        &workspace_root,
    )?;
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

/// An export directory: empty is an error, `~` and `~/` expand to the home
/// directory, and any other relative path is taken from the workspace root.
fn normalize_export_dir(
    value: Option<String>,
    key: &str,
    source_path: &Path,
    workspace_root: &Path,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(ConfigError::config_shape(
            source_path,
            format!("{key} cannot be empty; remove it or set a directory"),
        ));
    }
    let expanded = match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .ok_or_else(|| {
                    ConfigError::config_shape(
                        source_path,
                        format!("{key} '{value}' uses '~', but HOME is not set"),
                    )
                })?;
            PathBuf::from(home)
                .join(rest.trim_start_matches('/'))
                .display()
                .to_string()
        }
        _ => value.to_string(),
    };
    Ok(Some(
        absolutize(workspace_root, &expanded).display().to_string(),
    ))
}

/// `build_group` only means something to cargo; on any other artifact kind
/// it is a mistake rather than a key to ignore.
fn reject_non_rust_build_groups(raw: &raw::RawBuildConfig) -> Result<(), ConfigError> {
    let Some(artifact) = raw.artifacts.iter().find(|artifact| {
        artifact.build_group.is_some()
            && !matches!(artifact.definition, raw::RawArtifactDefinition::Rust { .. })
    }) else {
        return Ok(());
    };
    Err(ConfigError::ConfigShape {
        path: raw
            .source_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string()),
        message: format!(
            "artifact '{}' sets build_group, which is only supported for kind = \"rust\" artifacts",
            artifact.id
        ),
    })
}

/// `cargo_git_sources` maps cargo git dependencies, so it only applies to
/// `kind = "rust"` artifacts.
fn reject_non_rust_cargo_git_sources(raw: &raw::RawBuildConfig) -> Result<(), ConfigError> {
    let Some(artifact) = raw.artifacts.iter().find(|artifact| {
        !artifact.cargo_git_sources.is_empty()
            && !matches!(artifact.definition, raw::RawArtifactDefinition::Rust { .. })
    }) else {
        return Ok(());
    };
    Err(ConfigError::ConfigShape {
        path: raw
            .source_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string()),
        message: format!(
            "artifact '{}' sets cargo_git_sources, which is only supported for kind = \"rust\" artifacts",
            artifact.id
        ),
    })
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
