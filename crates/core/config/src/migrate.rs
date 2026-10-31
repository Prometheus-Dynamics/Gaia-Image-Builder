//! `gaia migrate`: rewrites a build's v2 files into config v3 and proves
//! the rewrite builds the same thing.
//!
//! The files are the build file and every local file it extends or imports,
//! followed whatever their `when` (a file a selection skips today may be
//! selected tomorrow). Files read from an import source's checkout belong to
//! that source and are left alone.
//!
//! Each build is resolved before and after the rewrite: with no preset,
//! with each preset its files declare, and every variant of each. The specs
//! must be equal apart from what only describes the files (their v2 note,
//! the line numbers in unknown-key warnings). Otherwise every file is put
//! back as it was and the first difference is reported.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use gaia_spec::ResolvedBuildSpec;

use crate::v3::{self, ConfigVersion};
use crate::{ConfigError, ResolveOptions, resolve_variant_specs};

/// A config file of a build and its config version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFileVersion {
    pub path: PathBuf,
    pub v3: bool,
}

/// What a migration did, or would do with `check_only`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Every local file of the builds, in the order they were found.
    pub files: Vec<ConfigFileVersion>,
    /// The files rewritten to v3 (none with `check_only`).
    pub migrated: Vec<PathBuf>,
    /// The resolutions compared, as `<build> [preset <name>]`.
    pub compared: Vec<String>,
}

impl MigrationReport {
    /// The files still in v2.
    pub fn v2_files(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .filter(|file| !file.v3)
            .map(|file| file.path.as_path())
    }
}

/// Migrates the local files of `builds` to config v3, or with
/// `check_only` only lists them with their versions.
pub fn migrate_builds(builds: &[String], check_only: bool) -> Result<MigrationReport, ConfigError> {
    let mut report = MigrationReport::default();
    let mut seen = BTreeSet::new();
    for build in builds {
        let entry = crate::load::resolve_build_path(build)?;
        collect_files(&entry, &mut seen, &mut report.files)?;
    }
    if check_only || report.v2_files().next().is_none() {
        return Ok(report);
    }

    let mut resolutions = Vec::new();
    for build in builds {
        for options in selections(build)? {
            resolutions.push((build.as_str(), options));
        }
    }
    let mut before = Vec::new();
    for (build, options) in &resolutions {
        report.compared.push(describe(build, options));
        before.push(resolve_for_comparison(build, options).map_err(|error| {
            migration_error(format!(
                "{} does not resolve before migrating, so the migration cannot be checked: {error}",
                describe(build, options)
            ))
        })?);
    }

    // Rewrite every v2 file in memory first: one that cannot be rewritten
    // leaves all of them untouched.
    let mut rewrites = Vec::new();
    for path in report.v2_files() {
        let text =
            fs::read_to_string(path).map_err(|error| ConfigError::config_read(path, error))?;
        let migrated = v3::migrate::migrate_config_text(&text)
            .map_err(|message| ConfigError::config_shape(path, message))?;
        rewrites.push((path.to_path_buf(), text, migrated));
    }
    for (path, _, migrated) in &rewrites {
        if let Err(error) = fs::write(path, migrated) {
            restore(&rewrites);
            return Err(ConfigError::config_read(path, error));
        }
    }

    for ((build, options), before) in resolutions.iter().zip(&before) {
        let after = match resolve_for_comparison(build, options) {
            Ok(after) => after,
            Err(error) => {
                restore(&rewrites);
                return Err(migration_error(format!(
                    "{} does not resolve after migrating, so nothing was changed: {error}",
                    describe(build, options)
                )));
            }
        };
        if let Some(difference) = first_difference(before, &after) {
            restore(&rewrites);
            return Err(migration_error(format!(
                "{} resolves differently after migrating ({difference}), so nothing was changed",
                describe(build, options)
            )));
        }
    }
    report.migrated = rewrites.into_iter().map(|(path, _, _)| path).collect();
    for file in &mut report.files {
        file.v3 = true;
    }
    Ok(report)
}

fn migration_error(message: String) -> ConfigError {
    ConfigError::ConfigMigrate { message }
}

fn restore(rewrites: &[(PathBuf, String, String)]) {
    for (path, original, _) in rewrites {
        if let Err(error) = fs::write(path, original) {
            tracing::error!(path = %path.display(), %error, "could not restore a config file after a failed migration");
        }
    }
}

fn describe(build: &str, options: &ResolveOptions) -> String {
    match &options.preset {
        Some(preset) => format!("build '{build}' with preset '{preset}'"),
        None => format!("build '{build}'"),
    }
}

/// No preset, then each preset the build's files declare.
fn selections(build: &str) -> Result<Vec<ResolveOptions>, ConfigError> {
    let merged = crate::load_merged(build, &ResolveOptions::default())?;
    let mut all = vec![ResolveOptions::default()];
    all.extend(merged.raw.presets.keys().map(|preset| ResolveOptions {
        preset: Some(preset.clone()),
        ..ResolveOptions::default()
    }));
    Ok(all)
}

/// Every variant's spec, without what only describes the files.
fn resolve_for_comparison(
    build: &str,
    options: &ResolveOptions,
) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
    let mut specs = resolve_variant_specs(build, options)?;
    for spec in &mut specs {
        spec.metadata.config_v2_files.clear();
        // Unknown-key warnings carry line numbers, which move.
        spec.metadata.config_warnings.clear();
        // v3 has one export dir; v2's two resolve to the one that wins.
        spec.export.image_dir = spec.export.configured_dir().map(ToString::to_string);
        spec.export.workspace_dir = None;
    }
    Ok(specs)
}

/// Where the specs first differ, as a JSON path with both values; `None`
/// when they differ only in a commit's `-dirty` mark.
fn first_difference(before: &[ResolvedBuildSpec], after: &[ResolvedBuildSpec]) -> Option<String> {
    if before.len() != after.len() {
        return Some(format!(
            "{} variant(s) before, {} after",
            before.len(),
            after.len()
        ));
    }
    before
        .iter()
        .zip(after)
        .filter(|(before, after)| before != after)
        .find_map(|(before, after)| {
            let before = gaia_spec::fingerprint::json_value(before);
            let after = gaia_spec::fingerprint::json_value(after);
            json_difference("", &before, &after)
        })
}

fn json_difference(
    path: &str,
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> Option<String> {
    use serde_json::Value;
    if before == after {
        return None;
    }
    match (before, after) {
        // Rewriting the files dirties a git checkout, which a commit token
        // (`${project.commit}`, `${source.<id>.commit}`) reports.
        (Value::String(before), Value::String(after))
            if after.strip_suffix("-dirty") == Some(before.as_str()) =>
        {
            None
        }
        (Value::Object(before_map), Value::Object(after_map)) => {
            for (key, value) in before_map {
                let child = format!("{path}.{key}");
                match after_map.get(key) {
                    Some(other) => {
                        if let Some(found) = json_difference(&child, value, other) {
                            return Some(found);
                        }
                    }
                    None => return Some(format!("`{}` is gone", &child[1..])),
                }
            }
            after_map
                .keys()
                .find(|key| !before_map.contains_key(*key))
                .map(|key| format!("`{}.{key}` is new", path.trim_start_matches('.')))
        }
        (Value::Array(before_items), Value::Array(after_items))
            if before_items.len() == after_items.len() =>
        {
            before_items
                .iter()
                .zip(after_items)
                .enumerate()
                .find_map(|(index, (item, other))| {
                    json_difference(&format!("{path}[{index}]"), item, other)
                })
        }
        _ => {
            let shorten = |value: &Value| {
                let text = value.to_string();
                if text.len() > 120 {
                    format!("{}...", &text[..text.floor_char_boundary(120)])
                } else {
                    text
                }
            };
            Some(format!(
                "`{}` was {}, is {}",
                path.trim_start_matches('.'),
                shorten(before),
                shorten(after)
            ))
        }
    }
}

/// The local files `path` loads, depth first, each once.
fn collect_files(
    path: &Path,
    seen: &mut BTreeSet<PathBuf>,
    files: &mut Vec<ConfigFileVersion>,
) -> Result<(), ConfigError> {
    let canonical =
        fs::canonicalize(path).map_err(|error| ConfigError::config_path(path, error))?;
    if !seen.insert(canonical.clone()) {
        return Ok(());
    }
    let text = fs::read_to_string(&canonical)
        .map_err(|error| ConfigError::config_read(&canonical, error))?;
    let mut value: toml::Value = toml::from_str(&text)
        .map_err(|error| ConfigError::config_parse_at(&canonical, &text, &error))?;
    let (version, _) = v3::read_version(&canonical, &text, &mut value)?;
    files.push(ConfigFileVersion {
        path: canonical.clone(),
        v3: version == ConfigVersion::V3,
    });
    let dir = canonical
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut referenced = Vec::new();
    if let Some(extends) = value.get("extends").and_then(toml::Value::as_str) {
        referenced.push(extends.to_string());
    }
    for import in value
        .get("imports")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        match import {
            toml::Value::String(path) => referenced.push(path.clone()),
            toml::Value::Table(table) if !table.contains_key("source") => {
                if let Some(path) = table.get("path").and_then(toml::Value::as_str) {
                    referenced.push(path.to_string());
                }
            }
            _ => {}
        }
    }
    for reference in referenced {
        let Some(local) = local_path(&dir, &reference) else {
            continue;
        };
        collect_files(&local, seen, files)?;
    }
    Ok(())
}

/// A file reference as a local path: relative to `dir`, or `@self/...`;
/// `None` for a path read from an import source.
fn local_path(dir: &Path, reference: &str) -> Option<PathBuf> {
    if reference.starts_with("@source:") {
        return None;
    }
    let reference = match reference.strip_prefix("@self") {
        Some(rest) => rest.trim_start_matches('/'),
        None => reference,
    };
    let path = Path::new(reference);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    })
}
