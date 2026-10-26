use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;
use crate::raw_assembly::RawAssemblyPackageConfig;

use super::resolver;

/// The config interpolation of one `[[image.assembly.packages]]` entry: its
/// paths, its control fields (`Version = "${build.version}"`) and scripts.
pub(super) fn interpolate_assembly_package(
    mut package: RawAssemblyPackageConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawAssemblyPackageConfig {
    text(&mut package.id, raw, env);
    opt(&mut package.format, raw, env);
    text(&mut package.tree, raw, env);
    text(&mut package.dest, raw, env);
    all(&mut package.allowed_prefixes, raw, env);

    let control = &mut package.control;
    opt(&mut control.package, raw, env);
    opt(&mut control.version, raw, env);
    opt(&mut control.architecture, raw, env);
    opt(&mut control.maintainer, raw, env);
    opt(&mut control.description, raw, env);
    all(&mut control.depends, raw, env);
    opt(&mut control.section, raw, env);
    opt(&mut control.priority, raw, env);
    opt(&mut control.source, raw, env);
    opt(&mut control.homepage, raw, env);
    for value in control.extra.values_mut() {
        text(value, raw, env);
    }

    let scripts = &mut package.scripts;
    all(&mut scripts.services, raw, env);
    opt(&mut scripts.postinst, raw, env);
    opt(&mut scripts.preinst, raw, env);
    opt(&mut scripts.prerm, raw, env);
    opt(&mut scripts.postrm, raw, env);
    all(&mut scripts.conffiles, raw, env);

    if let Some(device) = &mut package.device {
        opt(&mut device.toolchain, raw, env);
        opt(&mut device.sysroot, raw, env);
        opt(&mut device.max_glibc, raw, env);
    }
    package
}

fn text(value: &mut String, raw: &RawBuildConfig, env: &ResolvedEnvironment) {
    *value = resolver::interpolate_string(std::mem::take(value), raw, env);
}

fn opt(value: &mut Option<String>, raw: &RawBuildConfig, env: &ResolvedEnvironment) {
    if let Some(value) = value {
        text(value, raw, env);
    }
}

fn all(values: &mut [String], raw: &RawBuildConfig, env: &ResolvedEnvironment) {
    for value in values {
        text(value, raw, env);
    }
}
