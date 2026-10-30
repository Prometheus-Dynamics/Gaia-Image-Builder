//! `[[variants]]`: validation of the declared variants, and one resolved
//! spec per variant.
//!
//! A variant's spec is the base config with the variant's overrides applied,
//! then the command line's `--set` overrides. The CLI wins over the variant,
//! and the variant wins over the base file and its presets.
//!
//! The build file is loaded and merged once (`VariantBuild`). A variant
//! whose presets and overrides do not change what the loader reads takes
//! that merged config as it is; only a variant that does (an `input.*`, a
//! build target, an import source path) loads the build again for itself.

use std::collections::{BTreeMap, BTreeSet};

use gaia_spec::{DEFAULT_VARIANT_ID, ResolvedBuildSpec, is_valid_variant_id};

use crate::overrides::{source_path_override_id, variant_override_key_problem};
use crate::raw::{RawBuildConfig, RawVariantConfig};
use crate::{
    ConfigError, MergedBuild, ResolveOptions, ResolvedBuild, VariantResolution, load_merged,
    resolve_merged, undeclared_source_error,
};

/// Checks the declared `[[variants]]` of a merged build config: ids are
/// well-formed, unique and not `default`, and every override key names
/// something the build can apply.
pub(crate) fn validate_declared(raw: &RawBuildConfig) -> Result<(), ConfigError> {
    let path = raw
        .source_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| raw.build_name.clone());
    let shape = |message: String| ConfigError::ConfigShape {
        path: path.clone(),
        message,
    };
    let mut seen = BTreeSet::new();
    for variant in &raw.variants {
        let id = variant.id.as_str();
        if !is_valid_variant_id(id) {
            return Err(shape(format!(
                "variant id '{id}' must use lowercase letters, digits and dashes, and start with a letter or digit"
            )));
        }
        if id == DEFAULT_VARIANT_ID {
            return Err(shape(format!(
                "variant id '{DEFAULT_VARIANT_ID}' is reserved for the base config"
            )));
        }
        if !seen.insert(id) {
            return Err(shape(format!("variant '{id}' is declared more than once")));
        }
        for preset in variant_presets(variant) {
            if !raw.presets.contains_key(&preset) {
                return Err(shape(format!(
                    "variant '{id}' names preset '{preset}', which the build does not define"
                )));
            }
        }
        for (key, _) in &variant.overrides {
            if let Some(problem) = variant_override_key_problem(raw, key) {
                return Err(shape(format!("variant '{id}': {problem}")));
            }
        }
    }
    Ok(())
}

/// The presets a declared variant applies, in order: `preset`, then
/// `presets`.
pub(crate) fn variant_presets(variant: &RawVariantConfig) -> Vec<String> {
    variant
        .preset
        .iter()
        .chain(variant.presets.iter())
        .cloned()
        .collect()
}

/// Whether the loader or the merge reads an override key. A variant that
/// sets such a key may load different files, or different imports, than the
/// base build, so it loads the build again.
pub(crate) fn loader_reads_override(key: &str) -> bool {
    key.starts_with("input.")
        || key.starts_with("inputs.")
        || source_path_override_id(key).is_some()
        || matches!(
            key,
            "build.target"
                | "build.profile"
                | "build.branch"
                | "workspace.root_dir"
                | "policy.providers.git.timeout_seconds"
        )
}

/// A build file loaded and merged once, from which every variant of it is
/// resolved. The base options are those the build was loaded with; a
/// variant's own overrides and presets come on top of them.
pub struct VariantBuild {
    build: String,
    options: ResolveOptions,
    base: MergedBuild,
}

impl VariantBuild {
    /// Loads and merges the build under `options`.
    pub fn load(build: &str, options: &ResolveOptions) -> Result<Self, ConfigError> {
        Ok(Self {
            build: build.to_string(),
            options: options.clone(),
            base: load_merged(build, options)?,
        })
    }

    /// The build file name, as given to [`VariantBuild::load`].
    pub fn build(&self) -> &str {
        &self.build
    }

    /// The options the build was loaded with.
    pub fn options(&self) -> &ResolveOptions {
        &self.options
    }

    /// One spec per variant, see [`resolve_variant_specs`].
    pub fn resolve(&self) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
        self.resolve_scoped(&BTreeSet::new())
    }

    /// [`VariantBuild::resolve`] where every variant checks out the sources
    /// in `scoped_sources` for itself, see [`resolve_variant_specs_scoped`].
    pub fn resolve_scoped(
        &self,
        scoped_sources: &BTreeSet<String>,
    ) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
        let options = &self.options;
        let is_selected = |id: &str| {
            options.selected_variants.is_empty()
                || options.selected_variants.iter().any(|name| name == id)
        };
        let base_scope = VariantResolution {
            id: Some(DEFAULT_VARIANT_ID.to_string()),
            scoped_sources: scoped_sources.clone(),
        };
        let base = resolve_merged(&self.base, &self.build, options, &base_scope)?;
        let declared = base.spec.selection.variants.clone();
        if declared.is_empty() {
            return Ok(vec![base.into_checked_spec()?]);
        }
        // What the selected specs declare, and which `--set` source overrides
        // some of them apply or do not.
        let mut selected_declared = BTreeSet::new();
        let mut applied = BTreeSet::new();
        let mut unapplied = BTreeSet::new();
        let mut track = |resolved: &ResolvedBuild| {
            selected_declared.extend(resolved.declared_sources.iter().cloned());
            for (key, _) in &options.explicit_overrides {
                if resolved.undeclared_sources.iter().any(|(k, _)| k == key) {
                    unapplied.insert(key.clone());
                } else {
                    applied.insert(key.clone());
                }
            }
        };
        if is_selected(DEFAULT_VARIANT_ID) {
            track(&base);
        }
        let mut base = base.spec;
        base.selection.variant = Some(DEFAULT_VARIANT_ID.to_string());
        let mut specs = vec![base];
        for variant in declared {
            // Variant overrides first, so the command line's `--set` (applied
            // later) wins over them.
            let mut explicit_overrides = variant.overrides.clone();
            explicit_overrides.extend(options.explicit_overrides.iter().cloned());
            let variant_options = ResolveOptions {
                explicit_overrides,
                variant_presets: variant.presets.clone(),
                ..options.clone()
            };
            let scope = VariantResolution {
                id: Some(variant.id.clone()),
                scoped_sources: scoped_sources.clone(),
            };
            let resolved = self.resolve_variant(&variant.overrides, &variant_options, &scope)?;
            // A source the variant's own file names must be declared by it.
            if let Some((key, _)) = variant.overrides.iter().find(|(key, _)| {
                resolved
                    .undeclared_sources
                    .iter()
                    .any(|(undeclared, _)| undeclared == key)
            }) {
                return Err(undeclared_source_error(
                    key,
                    &format!("variant '{}'", variant.id),
                    resolved.declared_sources.clone(),
                ));
            }
            if is_selected(&variant.id) {
                track(&resolved);
            }
            let mut spec = resolved.spec;
            spec.selection.variant = Some(variant.id.clone());
            specs.push(spec);
        }
        // An override only needs to be applied by one selected spec.
        if let Some(key) = unapplied.iter().find(|key| !applied.contains(*key)) {
            return Err(undeclared_source_error(
                key,
                "any selected variant",
                selected_declared.into_iter().collect(),
            ));
        }
        Ok(specs)
    }

    /// Resolves one declared variant. `variant_options` carries the variant's
    /// presets and its overrides ahead of the command line's.
    fn resolve_variant(
        &self,
        overrides: &[(String, String)],
        variant_options: &ResolveOptions,
        scope: &VariantResolution,
    ) -> Result<ResolvedBuild, ConfigError> {
        let reads_load_keys = !variant_options.variant_presets.is_empty()
            || overrides.iter().any(|(key, _)| loader_reads_override(key));
        if reads_load_keys {
            let merged = load_merged(&self.build, variant_options)?;
            resolve_merged(&merged, &self.build, variant_options, scope)
        } else {
            resolve_merged(&self.base, &self.build, variant_options, scope)
        }
    }
}

/// Resolves the build once per variant, in order: the base config first (as
/// variant `default`), then each declared variant.
///
/// A build with no `[[variants]]` yields its one spec with no variant set.
/// Every spec carries the declared variants (`selection.variants`) so the
/// caller can plan them together.
pub fn resolve_variant_specs(
    build: &str,
    options: &ResolveOptions,
) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
    VariantBuild::load(build, options)?.resolve()
}

/// [`resolve_variant_specs`] where every variant checks out the sources in
/// `scoped_sources` for itself (`sources/variants/<variant>/<id>`), so the
/// variants' checkouts, and what reads them, differ. A variant's presets
/// replace the selected preset, then its overrides apply.
///
/// A `--set sources.<id>.path` names a source that each variant may or may
/// not declare: it applies to the variants that do. It is an error only when
/// no selected variant declares it (the base counts as selected when the
/// selection is empty or names `default`).
pub fn resolve_variant_specs_scoped(
    build: &str,
    options: &ResolveOptions,
    scoped_sources: &BTreeSet<String>,
) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
    VariantBuild::load(build, options)?.resolve_scoped(scoped_sources)
}

/// The output files of a resolved spec, as paths. Two variants whose specs
/// name the same path would write the same file.
///
/// The paths are what the spec names: `collect_dir/archive_name` for a raw
/// disk image, the `output` template of each assembly disk, archive and
/// filesystem, and `export_dir/<file name>` for each of them when exports go
/// to a configured directory. An image with no `archive_name` is written
/// under a name the provider derives from the display name and version, so
/// that name stands in for it.
fn output_paths(spec: &ResolvedBuildSpec) -> Vec<String> {
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .unwrap_or_else(|| gaia_spec::default_image_collect_dir(&spec.workspace));
    let collect_dir = collect_dir.trim_end_matches('/');
    let mut outputs = Vec::new();
    match &spec.image.output.archive_name {
        Some(name) => outputs.push(format!("{collect_dir}/{name}")),
        None => outputs.push(format!(
            "{collect_dir}/<{} {}>",
            spec.identity.display_name,
            spec.identity.version.as_deref().unwrap_or("")
        )),
    }
    if let Some(assembly) = &spec.image.assembly {
        outputs.extend(
            assembly
                .disks
                .iter()
                .map(|disk| disk.output.as_str().to_string()),
        );
        outputs.extend(
            assembly
                .archives
                .iter()
                .map(|archive| archive.output.as_str().to_string()),
        );
        outputs.extend(
            assembly
                .ocis
                .iter()
                .map(|oci| oci.dest.as_str().to_string()),
        );
        outputs.extend(
            assembly
                .packages
                .iter()
                .map(|package| package.dest.as_str().to_string()),
        );
        outputs.extend(
            assembly
                .filesystems
                .iter()
                .map(|filesystem| filesystem.output.as_str().to_string()),
        );
    }
    if let Some(export_dir) = spec.export.configured_dir() {
        let export_dir = export_dir.trim_end_matches('/');
        let exported = outputs
            .iter()
            .filter_map(|path| path.rsplit('/').next())
            .map(|file| format!("{export_dir}/{file}"))
            .collect::<Vec<_>>();
        outputs.extend(exported);
    }
    outputs.sort();
    outputs.dedup();
    outputs
}

/// Messages for every pair of variants that would write the same output
/// path (see `output_paths`). `specs` are the resolved, output-scoped
/// variant specs.
pub fn variant_output_collisions(specs: &[ResolvedBuildSpec]) -> Vec<String> {
    let mut by_path = BTreeMap::<String, Vec<&str>>::new();
    for spec in specs {
        let variant = spec
            .selection
            .variant
            .as_deref()
            .unwrap_or(DEFAULT_VARIANT_ID);
        for path in output_paths(spec) {
            let variants = by_path.entry(path).or_default();
            if !variants.contains(&variant) {
                variants.push(variant);
            }
        }
    }
    by_path
        .iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|(path, variants)| {
            format!(
                "variants {} would publish to the same image output {path}; give each variant its own image name or version, or its own image.output.archive_name",
                variants
                    .iter()
                    .map(|variant| format!("'{variant}'"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spec of a build resolved the way it was before the merged build
    /// was shared: the build file loaded again for each variant.
    fn full_load_specs(build: &str, options: &ResolveOptions) -> Vec<ResolvedBuildSpec> {
        let base_scope = VariantResolution {
            id: Some(DEFAULT_VARIANT_ID.to_string()),
            scoped_sources: BTreeSet::new(),
        };
        let base = crate::resolve_build_checked(build, options, &base_scope)
            .expect("base resolves")
            .into_checked_spec()
            .expect("base has no undeclared source");
        let declared = base.selection.variants.clone();
        let mut base = base;
        base.selection.variant = Some(DEFAULT_VARIANT_ID.to_string());
        let mut specs = vec![base];
        for variant in declared {
            let mut explicit_overrides = variant.overrides.clone();
            explicit_overrides.extend(options.explicit_overrides.iter().cloned());
            let variant_options = ResolveOptions {
                explicit_overrides,
                variant_presets: variant.presets.clone(),
                ..options.clone()
            };
            let scope = VariantResolution {
                id: Some(variant.id.clone()),
                scoped_sources: BTreeSet::new(),
            };
            let mut spec = crate::resolve_build_checked(build, &variant_options, &scope)
                .expect("variant resolves")
                .into_checked_spec()
                .expect("variant has no undeclared source");
            spec.selection.variant = Some(variant.id.clone());
            specs.push(spec);
        }
        specs
    }

    #[test]
    fn shared_build_resolves_the_same_specs_as_a_load_per_variant() {
        let dir = gaia_testkit::TempDir::new("variant-equivalence");
        let build_path = dir.join("build.toml");
        let root = dir.path().display();
        let contents = format!(
            r#"
build_name = "rel"
version = "1.0.0"
target = "x86_64-unknown-linux-gnu"

[inputs.rel]
kind = "string"
default = "2027"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app-source"
kind = "path"
path = "app"

[[artifacts]]
id = "jar"
kind = "rust"
package = "app-${{input.rel}}"
source = "app-source"
profile = "release"
target = "x86_64-unknown-linux-musl"
install_name = "app"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/app.bin"

[image]
kind = "starting-point"
rootfs_path = "seed/base.img"

[image.output]
archive_name = "rel.img"

[presets.rel2026]
overrides = [["input.rel", "2026"]]

[[variants]]
id = "by-input"
overrides = [["input.rel", "2025"]]

[[variants]]
id = "by-version"
overrides = [["build.version", "3.0.0"]]

[[variants]]
id = "by-preset"
preset = "rel2026"

[[variants]]
id = "by-target"
overrides = [["build.target", "aarch64-unknown-linux-gnu"]]

[[variants]]
id = "preset-then-override"
preset = "rel2026"
overrides = [["build.description", "from variant"]]
"#
        );
        std::fs::write(&build_path, contents).expect("write build file");
        let build = build_path.display().to_string();
        let option_sets = [
            ResolveOptions::default(),
            ResolveOptions {
                explicit_overrides: vec![
                    ("build.profile".into(), "debug".into()),
                    ("build.version".into(), "9.9.9".into()),
                ],
                ..ResolveOptions::default()
            },
        ];
        for options in &option_sets {
            let expected = full_load_specs(&build, options);
            assert_eq!(expected.len(), 6, "base and five variants");
            let shared = VariantBuild::load(&build, options)
                .and_then(|loaded| loaded.resolve())
                .expect("resolves");
            assert_eq!(shared, expected, "options {options:?}");
        }
    }

    #[test]
    fn variants_with_different_image_names_do_not_collide() {
        let mut first = ResolvedBuildSpec::new("rel");
        first.image.output.collect_dir = Some("/out/images".into());
        first.identity.display_name = "MyApp 2026".into();
        first.selection.variant = Some("rel2026".into());
        let mut second = first.clone();
        second.identity.display_name = "MyApp 2027".into();
        second.selection.variant = Some("rel2027".into());
        assert!(variant_output_collisions(&[first, second]).is_empty());
    }

    #[test]
    fn variants_publishing_the_same_file_are_reported() {
        let mut first = ResolvedBuildSpec::new("rel");
        first.image.output.collect_dir = Some("/out/images".into());
        first.selection.variant = Some("rel2026".into());
        let mut second = first.clone();
        second.selection.variant = Some("rel2027".into());
        let messages = variant_output_collisions(&[first, second]);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("'rel2026' and 'rel2027'"),
            "{messages:?}"
        );
    }

    #[test]
    fn variants_naming_the_same_archive_collide_whatever_else_differs() {
        let mut first = ResolvedBuildSpec::new("rel");
        first.image.output.collect_dir = Some("/out/images".into());
        first.image.output.archive_name = Some("disk.img".into());
        first.identity.display_name = "Alpha".into();
        first.identity.version = Some("1".into());
        first.selection.variant = Some("alpha".into());
        let mut second = first.clone();
        second.identity.display_name = "Beta".into();
        second.identity.version = Some("2".into());
        second.selection.variant = Some("beta".into());
        let messages = variant_output_collisions(&[first.clone(), second.clone()]);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("/out/images/disk.img"), "{messages:?}");

        second.image.output.archive_name = Some("other.img".into());
        assert!(variant_output_collisions(&[first, second]).is_empty());
    }

    #[test]
    fn exports_to_one_directory_collide_on_the_file_name() {
        let mut first = ResolvedBuildSpec::new("rel");
        first.image.output.collect_dir = Some("/out/a".into());
        first.image.output.archive_name = Some("disk.img".into());
        first.export.image_dir = Some("/exports".into());
        first.selection.variant = Some("a".into());
        let mut second = first.clone();
        second.image.output.collect_dir = Some("/out/b".into());
        second.selection.variant = Some("b".into());
        let messages = variant_output_collisions(&[first, second]);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("/exports/disk.img"), "{messages:?}");
    }

    #[test]
    fn the_same_variant_listing_an_output_twice_is_not_a_collision() {
        let mut only = ResolvedBuildSpec::new("rel");
        only.image.output.collect_dir = Some("/out/images".into());
        only.image.output.archive_name = Some("disk.img".into());
        only.export.image_dir = Some("/out/images".into());
        only.selection.variant = Some("only".into());
        assert!(variant_output_collisions(&[only]).is_empty());
    }
}
