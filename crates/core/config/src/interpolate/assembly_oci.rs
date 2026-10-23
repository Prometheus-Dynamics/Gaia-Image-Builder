use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;
use crate::raw_assembly::RawAssemblyOciConfig;

use super::resolver;

/// The config interpolation of one `[[image.assembly.oci]]` entry: its
/// paths, reference and image config strings.
pub(super) fn interpolate_assembly_oci(
    mut oci: RawAssemblyOciConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawAssemblyOciConfig {
    oci.id = resolver::interpolate_string(oci.id, raw, env);
    oci.dest = resolver::interpolate_string(oci.dest, raw, env);
    oci.format = oci
        .format
        .map(|value| resolver::interpolate_string(value, raw, env));
    oci.compression = oci
        .compression
        .map(|value| resolver::interpolate_string(value, raw, env));
    oci.reference = oci
        .reference
        .map(|value| resolver::interpolate_string(value, raw, env));
    for layer in &mut oci.layers {
        layer.tar = layer
            .tar
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
        layer.tree = layer
            .tree
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
    }
    let config = &mut oci.config;
    config.architecture = config
        .architecture
        .take()
        .map(|value| resolver::interpolate_string(value, raw, env));
    config.os = config
        .os
        .take()
        .map(|value| resolver::interpolate_string(value, raw, env));
    config.entrypoint = std::mem::take(&mut config.entrypoint)
        .into_iter()
        .map(|value| resolver::interpolate_string(value, raw, env))
        .collect();
    config.cmd = std::mem::take(&mut config.cmd)
        .into_iter()
        .map(|value| resolver::interpolate_string(value, raw, env))
        .collect();
    config.env = std::mem::take(&mut config.env)
        .into_iter()
        .map(|value| resolver::interpolate_string(value, raw, env))
        .collect();
    config.working_dir = config
        .working_dir
        .take()
        .map(|value| resolver::interpolate_string(value, raw, env));
    config.stop_signal = config
        .stop_signal
        .take()
        .map(|value| resolver::interpolate_string(value, raw, env));
    for value in config.labels.values_mut() {
        *value = resolver::interpolate_string(std::mem::take(value), raw, env);
    }
    oci
}
