//! `[[variants]]`: validation of the declared variants, and one resolved
//! spec per variant.
//!
//! A variant's spec is the base config with the variant's overrides applied,
//! then the command line's `--set` overrides. The CLI wins over the variant,
//! and the variant wins over the base file and its presets.

use std::collections::{BTreeMap, BTreeSet};

use gaia_spec::{DEFAULT_VARIANT_ID, ResolvedBuildSpec, is_valid_variant_id};

use crate::overrides::variant_override_key_problem;
use crate::raw::{RawBuildConfig, RawVariantConfig};
use crate::{ConfigError, ResolveOptions, VariantResolution, resolve_build};

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
    resolve_variant_specs_scoped(build, options, &BTreeSet::new())
}

/// [`resolve_variant_specs`] where every variant checks out the sources in
/// `scoped_sources` for itself (`sources/variants/<variant>/<id>`), so the
/// variants' checkouts, and what reads them, differ. A variant's presets
/// replace the selected preset, then its overrides apply.
pub fn resolve_variant_specs_scoped(
    build: &str,
    options: &ResolveOptions,
    scoped_sources: &BTreeSet<String>,
) -> Result<Vec<ResolvedBuildSpec>, ConfigError> {
    let base_scope = VariantResolution {
        id: Some(DEFAULT_VARIANT_ID.to_string()),
        scoped_sources: scoped_sources.clone(),
    };
    let mut base = resolve_build(build, options, &base_scope)?;
    let declared = base.selection.variants.clone();
    if declared.is_empty() {
        return Ok(vec![base]);
    }
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
        let mut spec = resolve_build(build, &variant_options, &scope)?;
        spec.selection.variant = Some(variant.id.clone());
        specs.push(spec);
    }
    Ok(specs)
}

/// Messages for every pair of variants that would publish to the same image
/// output: the same collect dir, export dir and assembly outputs, with the
/// same image name and version. `specs` are the resolved, output-scoped
/// variant specs.
pub fn variant_output_collisions(specs: &[ResolvedBuildSpec]) -> Vec<String> {
    let mut by_output = BTreeMap::<String, Vec<&str>>::new();
    for spec in specs {
        let variant = spec
            .selection
            .variant
            .as_deref()
            .unwrap_or(DEFAULT_VARIANT_ID);
        by_output
            .entry(publish_key(spec))
            .or_default()
            .push(variant);
    }
    by_output
        .values()
        .filter(|variants| variants.len() > 1)
        .map(|variants| {
            format!(
                "variants {} would publish to the same image output; give each variant its own image name or version, or its own image.output.archive_name",
                variants
                    .iter()
                    .map(|variant| format!("'{variant}'"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        })
        .collect()
}

/// What a variant publishes where. Two variants with equal keys write the
/// same file.
fn publish_key(spec: &ResolvedBuildSpec) -> String {
    let assembly_outputs = spec
        .image
        .assembly
        .as_ref()
        .map(|assembly| format!("{:?}|{:?}", assembly.disks, assembly.archives))
        .unwrap_or_default();
    format!(
        "collect={:?}|archive={:?}|name={}|version={:?}|export={:?}|assembly={}",
        spec.image.output.collect_dir,
        spec.image.output.archive_name,
        spec.identity.display_name,
        spec.identity.version,
        spec.export.configured_dir(),
        assembly_outputs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_with_different_image_names_do_not_collide() {
        let mut first = ResolvedBuildSpec::new("pv");
        first.image.output.collect_dir = Some("/out/images".into());
        first.identity.display_name = "PhotonVision 2026".into();
        first.selection.variant = Some("pv2026".into());
        let mut second = first.clone();
        second.identity.display_name = "PhotonVision 2027".into();
        second.selection.variant = Some("pv2027".into());
        assert!(variant_output_collisions(&[first, second]).is_empty());
    }

    #[test]
    fn variants_publishing_the_same_file_are_reported() {
        let mut first = ResolvedBuildSpec::new("pv");
        first.image.output.collect_dir = Some("/out/images".into());
        first.selection.variant = Some("pv2026".into());
        let mut second = first.clone();
        second.selection.variant = Some("pv2027".into());
        let messages = variant_output_collisions(&[first, second]);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("'pv2026' and 'pv2027'"),
            "{messages:?}"
        );
    }
}
