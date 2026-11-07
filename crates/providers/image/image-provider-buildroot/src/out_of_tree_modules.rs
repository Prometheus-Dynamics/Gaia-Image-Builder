//! `[[image.rootfs.out_of_tree_modules]]`: Kbuild modules built against the
//! image's own kernel. Each becomes a package `gaia-kmod-<name>` in Gaia's
//! generated external tree, using Buildroot's `kernel-module` infrastructure
//! with the module's directory as a local site, so it is built with the
//! kernel's tree, config, `Module.symvers` and compiler and its `vermagic`
//! matches. `modules_install` puts it under `lib/modules/<release>/updates/`.
//! The package's cache key and rebuild record carry the directory's content
//! (see `generated_package_digests`), so an edited module rebuilds. The
//! directory is mounted read-only into Buildroot's container.
use super::*;
use crate::rebuild_inputs::path_content_digest;
use gaia_spec::OutOfTreeModuleSpec;

/// The out-of-tree modules of a Buildroot spec.
pub(crate) fn out_of_tree_modules(spec: &ResolvedBuildSpec) -> &[OutOfTreeModuleSpec] {
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => &buildroot.rootfs_out_of_tree_modules,
        _ => &[],
    }
}

/// The module's Kbuild directory, resolved like an external tree.
fn module_dir(spec: &ResolvedBuildSpec, module: &OutOfTreeModuleSpec) -> PathBuf {
    gaia_image_providers::resolve_tree_path(&spec.workspace, &module.path)
}

/// Writes each module's package into `package_dir` and returns the package
/// names. A module directory without a `Kbuild` or `Makefile` fails here,
/// before Buildroot runs.
pub(crate) fn write_module_packages(
    spec: &ResolvedBuildSpec,
    package_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let mut packages = Vec::new();
    for module in out_of_tree_modules(spec) {
        let dir = module_dir(spec, module);
        if !dir.join("Kbuild").is_file() && !dir.join("Makefile").is_file() {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                format!(
                    "out_of_tree_modules: '{}' names '{}', which has no Kbuild or Makefile",
                    module.name,
                    dir.display()
                ),
            ));
        }
        // Buildroot in a container rsyncs the directory from its own path:
        // only the generated `.mk` names it, so no argument makes it a mount.
        gaia_process::register_read_only_docker_mount(&dir);
        let package = module.package();
        let dest = package_dir.join(&package);
        fs::create_dir_all(&dest).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create '{}': {error}",
                dest.display()
            ))
        })?;
        for (file, text) in [
            ("Config.in".to_string(), config_in(module, &dir)),
            (format!("{package}.mk"), package_mk(module, &dir)),
        ] {
            fs::write(dest.join(&file), text).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to write '{}': {error}",
                    dest.join(&file).display()
                ))
            })?;
        }
        packages.push(package);
    }
    Ok(packages)
}

pub(crate) fn config_in(module: &OutOfTreeModuleSpec, dir: &Path) -> String {
    format!(
        "config {option}\n\tbool \"{package}\"\n\tdepends on BR2_LINUX_KERNEL\n\thelp\n\t  Written by Gaia: the out-of-tree kernel module in\n\t  {dir}, built against the image's kernel.\n",
        option = module.option(),
        package = module.package(),
        dir = dir.display()
    )
}

pub(crate) fn package_mk(module: &OutOfTreeModuleSpec, dir: &Path) -> String {
    let var = module
        .option()
        .trim_start_matches("BR2_PACKAGE_")
        .to_string();
    format!(
        "# Written by Gaia ([[image.rootfs.out_of_tree_modules]] '{name}').\n\
         {var}_VERSION = gaia\n\
         {var}_SITE = {dir}\n\
         {var}_SITE_METHOD = local\n\
         $(eval $(kernel-module))\n\
         $(eval $(generic-package))\n",
        name = module.name,
        dir = dir.display()
    )
}

/// Each module package with its directory's content digest.
pub(crate) fn module_package_digests(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    out_of_tree_modules(spec)
        .iter()
        .map(|module| {
            let digest = path_content_digest(None, &module_dir(spec, module));
            (module.package(), format!("kmod-v1:{digest}"))
        })
        .collect()
}

#[cfg(test)]
#[path = "out_of_tree_modules_tests.rs"]
mod tests;
