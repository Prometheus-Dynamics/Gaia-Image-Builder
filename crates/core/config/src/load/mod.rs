mod import_sources;
mod path_tokens;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use gaia_spec::ImportSourceSpec;

use crate::overrides::source_path_override_id;
use crate::raw::{RawBuildConfig, RawImportConfig};
use crate::{ConfigError, ResolveOptions};
use import_sources::ImportSources;
use path_tokens::rewrite_path_tokens;

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

/// Loads the build entrypoint and every file it extends or imports, and
/// reports the import sources that supplied files.
pub(crate) fn load_build_config(
    build: &str,
    options: &ResolveOptions,
) -> Result<LoadedBuildConfig, ConfigError> {
    tracing::debug!(build, "resolving build config path");
    let build_path = resolve_build_path(build)?;
    tracing::debug!(path = %build_path.display(), "loading build config");
    let entrypoint = fs::canonicalize(&build_path)
        .map_err(|error| ConfigError::config_path(&build_path, error))?;
    let workspace_root = import_workspace_root(&entrypoint, options)?;
    let path_overrides = options
        .explicit_overrides
        .iter()
        .filter_map(|(key, value)| {
            source_path_override_id(key)
                .map(|id| (id.to_string(), absolutize_from(&workspace_root, value)))
        })
        .collect();
    let mut sources = ImportSources::new(
        workspace_root,
        &entrypoint,
        path_overrides,
        options.resolve_unpinned_import_sources,
    );
    if let Some(seconds) = options
        .explicit_overrides
        .iter()
        .rev()
        .find(|(key, _)| key == "policy.providers.git.timeout_seconds")
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
    {
        sources.override_timeout(seconds);
    }
    declare_local_sources(&entrypoint, &mut sources, &mut BTreeSet::new());
    let mut loader = Loader {
        stack: Vec::new(),
        sources,
        build,
        options,
    };
    let config = loader.load(&entrypoint, None, true)?;
    tracing::debug!(
        build_name = %config.build_name,
        imports = config.imported_configs.len(),
        has_extends = config.extends_config.is_some(),
        "build config loaded"
    );
    let unused_import_sources = loader.sources.unused_references();
    Ok(LoadedBuildConfig {
        raw: config,
        import_sources: loader.sources.into_specs(),
        unused_import_sources,
    })
}

pub(crate) struct LoadedBuildConfig {
    pub(crate) raw: RawBuildConfig,
    /// Import sources that supplied files or `@source:` paths.
    pub(crate) import_sources: Vec<ImportSourceSpec>,
    /// Sources referenced as import sources somewhere in the local config
    /// files, but only by layers this selection does not use.
    pub(crate) unused_import_sources: Vec<String>,
}

struct Loader<'a> {
    stack: Vec<PathBuf>,
    sources: ImportSources,
    build: &'a str,
    options: &'a ResolveOptions,
}

impl Loader<'_> {
    /// Loads one config file and everything it extends or imports. `origin`
    /// is the import source the file was read from, `None` for local files.
    fn load(
        &mut self,
        path: &Path,
        origin: Option<&str>,
        entrypoint: bool,
    ) -> Result<RawBuildConfig, ConfigError> {
        let canonical_path =
            fs::canonicalize(path).map_err(|error| ConfigError::config_path(path, error))?;
        if self.stack.contains(&canonical_path) {
            let mut cycle = self
                .stack
                .iter()
                .map(|entry| entry.display().to_string())
                .collect::<Vec<_>>();
            cycle.push(canonical_path.display().to_string());
            return Err(ConfigError::ConfigImportCycle { cycle });
        }

        self.stack.push(canonical_path.clone());
        tracing::trace!(
            path = %canonical_path.display(),
            depth = self.stack.len(),
            import_source = origin,
            "reading config file"
        );
        let contents = fs::read_to_string(&canonical_path)
            .map_err(|error| ConfigError::config_read(&canonical_path, error))?;
        // An empty build file is never intended: most likely a write that
        // never finished, which would silently drop what the file provides.
        if contents.trim().is_empty() {
            return Err(ConfigError::config_shape(
                &canonical_path,
                "the file is empty; a config file must set something (was it truncated?)",
            ));
        }
        let mut value: toml::Value = toml::from_str(&contents)
            .map_err(|error| ConfigError::config_parse(&canonical_path, error))?;
        crate::gaia_version::check_required_gaia_version(&canonical_path, &value)?;
        let config_dir = canonical_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let sources = &mut self.sources;
        rewrite_path_tokens(&mut value, &config_dir, &mut |id| {
            sources.root(id, &canonical_path).map(Some)
        })?;
        if let Some(id) = origin {
            self.sources.record_file(id, &value, &contents);
        }
        validate_raw_toml_shape(&canonical_path, &value)?;
        let unknown_keys = crate::unknown_keys::unknown_config_keys(&value);
        let sets_nothing = value.as_table().is_some_and(toml::Table::is_empty);
        let mut raw: RawBuildConfig = value
            .try_into()
            .map_err(|error| ConfigError::config_parse(&canonical_path, error))?;
        raw.source_path = Some(canonical_path.clone());
        raw.unknown_keys = unknown_keys;
        raw.sets_nothing = sets_nothing;
        if raw.build_name.trim().is_empty() {
            raw.build_name = infer_build_name(&canonical_path);
        }

        let ensure_exists = |field: &'static str, reference: &str, resolved: &Path| {
            if resolved.is_file() {
                Ok(())
            } else {
                Err(ConfigError::ConfigReferenceMissing {
                    referenced_by: canonical_path.display().to_string(),
                    field,
                    reference: reference.to_string(),
                    resolved: resolved.display().to_string(),
                })
            }
        };
        if let Some(extends) = raw.extends.clone() {
            let extends_path = resolve_relative_config_path(&config_dir, &extends);
            if let Some(id) = origin {
                self.sources
                    .ensure_inside(id, &extends_path, &canonical_path)?;
            }
            ensure_exists("extends", &extends, &extends_path)?;
            tracing::trace!(
                path = %canonical_path.display(),
                extends = %extends_path.display(),
                "loading extended config"
            );
            raw.extends_config = Some(Box::new(self.load(&extends_path, origin, false)?));
        }
        let mut imported_configs = Vec::new();
        for import in raw.imports.clone() {
            // Merging drops an import whose `when` does not match, so do not
            // load it at all: nothing in it may trigger a source checkout or
            // `@source:` resolution for a layer that is not selected.
            if !self.import_applies(&raw, entrypoint, &import) {
                tracing::trace!(
                    import_source = import.source.as_deref(),
                    path = %import.path,
                    "skipping import whose when does not match"
                );
                continue;
            }
            let (import_path, import_origin) = match import.source.as_deref() {
                Some(id) => {
                    let path = self
                        .sources
                        .import_path(id, &import.path, &canonical_path)?;
                    (path, Some(id.to_string()))
                }
                None => {
                    let path = resolve_relative_config_path(&config_dir, &import.path);
                    if let Some(id) = origin {
                        self.sources.ensure_inside(id, &path, &canonical_path)?;
                    }
                    (path, origin.map(ToString::to_string))
                }
            };
            ensure_exists("import", &import.path, &import_path)?;
            tracing::trace!(
                path = %canonical_path.display(),
                import = %import_path.display(),
                "loading imported config"
            );
            let config = self.load(&import_path, import_origin.as_deref(), false)?;
            imported_configs.push(crate::raw::RawImportedConfig { import, config });
        }
        raw.imported_configs = imported_configs;

        self.stack.pop();
        Ok(raw)
    }

    /// Evaluates an import's `when` against the importing file exactly as
    /// `merge_config` will.
    fn import_applies(
        &self,
        raw: &RawBuildConfig,
        entrypoint: bool,
        import: &RawImportConfig,
    ) -> bool {
        let Some(when) = import.when.as_ref() else {
            return true;
        };
        let mut context = raw.clone();
        context.imports.clear();
        if entrypoint {
            context = crate::apply_preset_selection(context, self.build, self.options);
        }
        crate::merge::import_applies(&context, when)
    }
}

/// Registers the git sources declared by local config files: the
/// entrypoint, its `extends` chain and files imported by plain path.
/// Best effort: unreadable files are reported by the real load.
fn declare_local_sources(path: &Path, sources: &mut ImportSources, seen: &mut BTreeSet<PathBuf>) {
    let Ok(canonical) = fs::canonicalize(path) else {
        return;
    };
    if !seen.insert(canonical.clone()) {
        return;
    }
    let Some(mut value) = fs::read_to_string(&canonical)
        .ok()
        .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
    else {
        return;
    };
    let config_dir = canonical
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut referenced = BTreeSet::new();
    let rewritten = rewrite_path_tokens(&mut value, &config_dir, &mut |id| {
        referenced.insert(id.to_string());
        Ok(None)
    });
    if rewritten.is_err() {
        return;
    }
    sources.declare_from(&canonical, &value);
    for import in value
        .get("imports")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(id) = import.get("source").and_then(toml::Value::as_str) {
            referenced.insert(id.to_string());
        }
    }
    sources.note_references(referenced);
    let mut local = Vec::new();
    if let Some(extends) = value.get("extends").and_then(toml::Value::as_str) {
        local.push(extends.to_string());
    }
    for import in value
        .get("imports")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        match import {
            toml::Value::String(path) => local.push(path.clone()),
            toml::Value::Table(table) if !table.contains_key("source") => {
                if let Some(path) = table.get("path").and_then(toml::Value::as_str) {
                    local.push(path.to_string());
                }
            }
            _ => {}
        }
    }
    for path in local {
        declare_local_sources(
            &resolve_relative_config_path(&config_dir, &path),
            sources,
            seen,
        );
    }
}

/// Workspace root holding the import-source cache: the directory with the
/// entrypoint's nearest `Cargo.toml` (else the discovered build root),
/// adjusted by `workspace.root_dir` from `--set` or the entrypoint itself.
fn import_workspace_root(
    entrypoint: &Path,
    options: &ResolveOptions,
) -> Result<PathBuf, ConfigError> {
    let build_root = match entrypoint
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join("Cargo.toml").is_file())
    {
        Some(root) => root.to_path_buf(),
        None => discover_build_root()?,
    };
    let root_dir = options
        .explicit_overrides
        .iter()
        .rev()
        .find(|(key, _)| key == "workspace.root_dir")
        .map(|(_, value)| value.clone())
        .or_else(|| {
            fs::read_to_string(entrypoint)
                .ok()
                .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
                .and_then(|value| {
                    value
                        .get("workspace")?
                        .get("root_dir")?
                        .as_str()
                        .map(ToString::to_string)
                })
        })
        .filter(|root_dir| !root_dir.trim().is_empty());
    Ok(match root_dir {
        Some(root_dir) => absolutize_from(&build_root, &root_dir),
        None => build_root,
    })
}

fn absolutize_from(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
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
