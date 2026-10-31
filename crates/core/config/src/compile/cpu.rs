//! `[cpu]`: the board's CPU profile, applied to every Rust artifact built
//! for its architecture and to a Buildroot image.

use super::*;
use crate::raw::RawCpuConfig;
use std::collections::BTreeSet;

/// The ids of the Rust artifacts that set `cpu = false`.
pub(super) fn opt_outs(artifacts: &[RawArtifactConfig]) -> BTreeSet<String> {
    artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.definition,
                RawArtifactDefinition::Rust {
                    cpu: Some(false),
                    ..
                }
            )
        })
        .map(|artifact| artifact.id.clone())
        .collect()
}

/// Sets each Rust artifact's CPU flags and prepends the profile's Buildroot
/// symbols, so the image's own `config_overrides` still win. An unknown
/// profile was refused when the config was loaded.
pub(super) fn apply(cpu: &RawCpuConfig, opt_outs: &BTreeSet<String>, spec: &mut ResolvedBuildSpec) {
    let Some(profile) = cpu.profile.as_deref().and_then(gaia_spec::cpu_profile) else {
        return;
    };
    for artifact in &mut spec.artifacts {
        if opt_outs.contains(artifact.id.as_str()) {
            continue;
        }
        // An artifact without a target of its own builds for its
        // toolchain's.
        let target = artifact.target.clone().or_else(|| {
            let id = artifact.toolchain.as_deref()?;
            spec.toolchains
                .iter()
                .find(|toolchain| toolchain.id == id)
                .and_then(|toolchain| toolchain.rust_target.clone())
        });
        if let (ArtifactDefinition::Rust(rust), Some(target)) = (&mut artifact.definition, target) {
            rust.cpu = profile.for_rust_target(&target);
        }
    }
    if cpu.buildroot != Some(false)
        && let ImageDefinition::Buildroot(buildroot) = &mut spec.image.definition
    {
        let mut overrides: Vec<(String, String)> = profile
            .buildroot
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        overrides.append(&mut buildroot.config_overrides);
        buildroot.config_overrides = overrides;
    }
}
