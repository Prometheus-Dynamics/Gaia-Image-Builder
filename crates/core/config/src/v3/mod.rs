//! Config v3: a build file with `config = 3` at its top level.
//!
//! v3 changes the spelling of a build file, not what it builds. Each v3 file
//! is translated into the v2 shape right after it is parsed, so everything
//! after that (path tokens, merging, interpolation, compilation) reads one
//! shape, and a file and its `gaia migrate` output resolve to the same spec.
//! The changes (see `docs/design/config-v3.md`):
//! - one token syntax, `${...}` (see [`tokens`]);
//! - `[outputs]` for `workspace.out_dir`, `workspace.export_dir` and
//!   `[image.output]`;
//! - `[cache]` for the Buildroot ccache, package cache and download dir and
//!   Rust's sccache and shared target dir;
//! - choice fields checked as they are read (see [`enums`]).
//!
//! A v2 key or token in a v3 file is an error at its line that names the v3
//! spelling. Files without `config` are v2 and keep loading.

mod enums;
pub(crate) mod migrate;
pub(crate) mod tokens;

use std::path::Path;

use crate::ConfigError;
use crate::locate::{KeyLocations, at_offset};

/// The top-level key that sets a file's config version.
pub(crate) const VERSION_KEY: &str = "config";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigVersion {
    V2,
    V3,
}

/// A v3 key and the v2 key it is read as. `[outputs]` and `[cache]` are
/// only these; any other key in them is an error.
pub(crate) const MOVED_KEYS: &[(&str, &str)] = &[
    ("outputs.dir", "workspace.out_dir"),
    ("outputs.export_dir", "image.output.export_dir"),
    ("outputs.images_dir", "image.output.collect_dir"),
    ("outputs.archive_name", "image.output.archive_name"),
    ("outputs.disk", "image.output.disk"),
    ("outputs.report", "image.output.emit_report"),
    ("cache.ccache", "providers.buildroot.ccache"),
    ("cache.packages", "providers.buildroot.package_cache"),
    ("cache.downloads", "providers.buildroot.download_dir"),
    ("cache.sccache", "providers.rust.sccache"),
    (
        "cache.shared_target_dir",
        "providers.rust.shared_target_dir",
    ),
];

/// The `--set` and preset override keys v3 spells differently, and the
/// key each stands for. The v2 spellings keep working.
pub(crate) const OVERRIDE_KEYS: &[(&str, &str)] = &[
    ("outputs.dir", "workspace.out_dir"),
    ("outputs.export_dir", "image.output.export_dir"),
    ("outputs.images_dir", "image.output.collect_dir"),
    ("outputs.archive_name", "image.output.archive_name"),
    ("cache.downloads", "policy.providers.buildroot.download_dir"),
    (
        "cache.ccache.enabled",
        "policy.providers.buildroot.ccache.enabled",
    ),
    ("cache.ccache.dir", "policy.providers.buildroot.ccache.dir"),
    (
        "cache.ccache.max_size",
        "policy.providers.buildroot.ccache.max_size",
    ),
    (
        "cache.packages.enabled",
        "policy.providers.buildroot.package_cache.enabled",
    ),
    (
        "cache.packages.dir",
        "policy.providers.buildroot.package_cache.dir",
    ),
    (
        "cache.packages.system_dir",
        "policy.providers.buildroot.package_cache.system_dir",
    ),
    (
        "cache.packages.level",
        "policy.providers.buildroot.package_cache.level",
    ),
    (
        "cache.packages.project_dir",
        "policy.providers.buildroot.package_cache.project_dir",
    ),
    (
        "cache.packages.max_size",
        "policy.providers.buildroot.package_cache.max_size",
    ),
    ("cache.sccache", "policy.providers.rust.sccache"),
    (
        "cache.shared_target_dir",
        "policy.providers.rust.shared_target_dir",
    ),
];

/// v2 keys that only `workspace.export_dir` reads besides
/// [`MOVED_KEYS`]: v3 has one `outputs.export_dir`.
const V2_ONLY_KEYS: &[(&str, &str)] = &[("workspace.export_dir", "outputs.export_dir")];

/// The provider tables whose cache keys moved to `[cache]`. v2 accepted
/// them under every provider, though only Buildroot and Rust read them.
const PROVIDER_TABLES: &[&str] = &[
    "archive",
    "download",
    "go",
    "java",
    "node",
    "python",
    "buildroot",
    "starting_point",
    "rust",
];

/// What translating a file found that is checked after the merge: the
/// named-path aliases its tokens use, each with where it is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct V3FileUses {
    pub aliases: Vec<(String, String)>,
}

/// The version of the parsed file at `path` (`text` is its text), and for
/// a v3 file, `value` translated into the v2 shape.
pub(crate) fn read_version(
    path: &Path,
    text: &str,
    value: &mut toml::Value,
) -> Result<(ConfigVersion, V3FileUses), ConfigError> {
    let Some(table) = value.as_table_mut() else {
        return Ok((ConfigVersion::V2, V3FileUses::default()));
    };
    let Some(version) = table.remove(VERSION_KEY) else {
        return Ok((ConfigVersion::V2, V3FileUses::default()));
    };
    let locations = KeyLocations::scan(text);
    let error = |key: &str, message: String| {
        let at = locations.offset(key).map_or_else(
            || path.display().to_string(),
            |offset| at_offset(path, text, offset),
        );
        ConfigError::config_shape(path, format!("{at}: {message}"))
    };
    match version.as_integer() {
        Some(2) => return Ok((ConfigVersion::V2, V3FileUses::default())),
        Some(3) => {}
        _ => {
            return Err(error(
                VERSION_KEY,
                format!(
                    "config = {version} is not a config version; use 3 (or leave it out for v2)"
                ),
            ));
        }
    }
    reject_v2_keys(table, &error)?;
    move_keys(table, &error)?;
    enums::check(table, &error)?;
    let named_paths = get(table, "workspace.named_paths").and_then(toml::Value::as_array);
    for (index, named) in named_paths.into_iter().flatten().enumerate() {
        if let Some(alias) = named.get("alias").and_then(toml::Value::as_str)
            && tokens::alias_is_reserved(alias)
        {
            return Err(error(
                &format!("workspace.named_paths[{index}].alias"),
                format!(
                    "alias '{alias}' cannot be used: `${{{alias}}}` already means something else"
                ),
            ));
        }
    }
    let mut uses = V3FileUses::default();
    translate_strings(value, "", &locations, path, text, &mut uses)?;
    Ok((ConfigVersion::V3, uses))
}

fn get<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut segments = key.split('.');
    let mut value = table.get(segments.next()?)?;
    for segment in segments {
        value = value.as_table()?.get(segment)?;
    }
    Some(value)
}

/// A v2 key a v3 file sets, named with its v3 spelling.
fn reject_v2_keys(
    table: &toml::Table,
    error: &impl Fn(&str, String) -> ConfigError,
) -> Result<(), ConfigError> {
    let renamed = MOVED_KEYS
        .iter()
        .map(|(v3, v2)| (*v2, *v3))
        .chain(V2_ONLY_KEYS.iter().copied());
    for (v2, v3) in renamed {
        if v2.starts_with("providers.") {
            continue;
        }
        if get(table, v2).is_some() {
            return Err(error(v2, format!("`{v2}` is `{v3}` in config v3")));
        }
    }
    if let Some(output) = get(table, "image.output").and_then(toml::Value::as_table)
        && let Some(key) = output.keys().next()
    {
        let v2 = format!("image.output.{key}");
        let v3 = MOVED_KEYS
            .iter()
            .find(|(_, moved)| *moved == v2)
            .map_or("a key of `[outputs]`", |(v3, _)| v3);
        return Err(error(&v2, format!("`{v2}` is `{v3}` in config v3")));
    }
    for provider in PROVIDER_TABLES {
        for (key, v3) in [
            ("ccache", "cache.ccache"),
            ("package_cache", "cache.packages"),
            ("download_dir", "cache.downloads"),
            ("sccache", "cache.sccache"),
            ("shared_target_dir", "cache.shared_target_dir"),
        ] {
            let v2 = format!("providers.{provider}.{key}");
            if get(table, &v2).is_some() {
                return Err(error(&v2, format!("`{v2}` is `{v3}` in config v3")));
            }
        }
    }
    Ok(())
}

/// Moves `[outputs]` and `[cache]` to their v2 keys.
fn move_keys(
    table: &mut toml::Table,
    error: &impl Fn(&str, String) -> ConfigError,
) -> Result<(), ConfigError> {
    for section in ["outputs", "cache"] {
        let Some(moved) = table.remove(section) else {
            continue;
        };
        let Some(moved) = moved.as_table() else {
            return Err(error(section, format!("`{section}` must be a table")));
        };
        for (key, value) in moved {
            let v3 = format!("{section}.{key}");
            let Some((_, v2)) = MOVED_KEYS.iter().find(|(name, _)| *name == v3) else {
                let known = MOVED_KEYS
                    .iter()
                    .filter_map(|(name, _)| name.strip_prefix(&format!("{section}.")))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(error(
                    &v3,
                    format!("`{v3}` is not a key of `[{section}]`; it takes {known}"),
                ));
            };
            if !insert(table, v2, value.clone()) {
                let parent = v2.rsplit_once('.').map_or(*v2, |(parent, _)| parent);
                return Err(error(
                    parent,
                    format!("`{parent}` must be a table to hold `{v3}`"),
                ));
            }
        }
    }
    Ok(())
}

/// Sets the dotted `key` in `table`, creating the tables on the way;
/// `false` when a value on the way is not a table.
fn insert(table: &mut toml::Table, key: &str, value: toml::Value) -> bool {
    let (parents, last) = key.rsplit_once('.').unwrap_or(("", key));
    let mut current = table;
    for segment in parents.split('.').filter(|segment| !segment.is_empty()) {
        let next = current
            .entry(segment)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(next) = next.as_table_mut() else {
            return false;
        };
        current = next;
    }
    current.insert(last.to_string(), value);
    true
}

/// Translates every string value, recording where each one is for errors.
/// `key` is the value's dotted path as [`KeyLocations`] names it.
fn translate_strings(
    value: &mut toml::Value,
    key: &str,
    locations: &KeyLocations,
    path: &Path,
    text: &str,
    uses: &mut V3FileUses,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(string) => {
            let mut found = tokens::V3Tokens::default();
            let at = || locate(locations, key, path, text);
            match tokens::v3_string_to_v2(string, &mut found) {
                Ok(translated) => *string = translated,
                Err(message) => {
                    return Err(ConfigError::config_shape(
                        path,
                        format!("{}: {message}", at()),
                    ));
                }
            }
            uses.aliases
                .extend(found.aliases.into_iter().map(|alias| (alias, at())));
        }
        toml::Value::Array(items) => {
            let tables = items.iter().all(toml::Value::is_table) && !items.is_empty();
            for (index, item) in items.iter_mut().enumerate() {
                let item_key = if tables {
                    format!("{key}[{index}]")
                } else {
                    key.to_string()
                };
                translate_strings(item, &item_key, locations, path, text, uses)?;
            }
        }
        toml::Value::Table(table) => {
            for (name, item) in table.iter_mut() {
                let item_key = if key.is_empty() {
                    name.clone()
                } else {
                    format!("{key}.{name}")
                };
                translate_strings(item, &item_key, locations, path, text, uses)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `path:line:col` of the key `key`, or of its nearest enclosing table or
/// key that the file spells out; the bare path when none is.
fn locate(locations: &KeyLocations, key: &str, path: &Path, text: &str) -> String {
    let mut candidate = key;
    loop {
        if let Some(offset) = locations.offset(candidate) {
            return at_offset(path, text, offset);
        }
        let parent = match (candidate.rfind('.'), candidate.rfind('[')) {
            (Some(dot), Some(bracket)) => dot.max(bracket),
            (Some(dot), None) => dot,
            (None, Some(bracket)) => bracket,
            (None, None) => return path.display().to_string(),
        };
        candidate = &candidate[..parent];
    }
}

/// The files of a loaded build that are v2, and the named-path aliases
/// its v3 files use (with where), across every layer.
pub(crate) fn layer_versions(
    raw: &crate::raw::RawBuildConfig,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut v2_files = Vec::new();
    let mut aliases = Vec::new();
    let mut visit = vec![raw];
    while let Some(config) = visit.pop() {
        match config.config_version {
            Some(ConfigVersion::V2) => {
                if let Some(path) = &config.source_path {
                    let path = path.display().to_string();
                    if !v2_files.contains(&path) {
                        v2_files.push(path);
                    }
                }
            }
            Some(ConfigVersion::V3) => aliases.extend(config.v3_uses.aliases.iter().cloned()),
            None => {}
        }
        visit.extend(config.extends_config.as_deref());
        visit.extend(
            config
                .imported_configs
                .iter()
                .map(|imported| &imported.config),
        );
    }
    v2_files.sort();
    (v2_files, aliases)
}

/// An alias a v3 file's `${<alias>}` uses that the build does not declare.
pub(crate) fn check_aliases(
    aliases: &[(String, String)],
    workspace: &gaia_spec::WorkspaceSpec,
) -> Result<(), ConfigError> {
    for (alias, at) in aliases {
        if !workspace
            .named_paths
            .iter()
            .any(|named| named.alias == *alias)
        {
            let declared = workspace
                .named_paths
                .iter()
                .map(|named| format!("${{{}}}", named.alias))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ConfigError::ConfigShape {
                path: at.clone(),
                message: format!(
                    "`${{{alias}}}` is neither a token nor a `[[workspace.named_paths]]` alias (the build's aliases: {declared})"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
