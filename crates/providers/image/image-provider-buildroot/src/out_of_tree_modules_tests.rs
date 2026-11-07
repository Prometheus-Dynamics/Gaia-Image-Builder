use super::*;
use crate::buildroot_external::{
    generated_package_digests, materialize_buildroot_package_overrides,
};
use gaia_spec::BuildrootImageSpec;
use gaia_testkit::TempDir;

fn spec_with(root: &Path, modules: &[(&str, &str)]) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("out-of-tree-modules");
    spec.workspace.root_dir = root.display().to_string();
    spec.image.definition = ImageDefinition::Buildroot(BuildrootImageSpec {
        rootfs_out_of_tree_modules: modules
            .iter()
            .map(|(name, path)| OutOfTreeModuleSpec {
                name: name.to_string(),
                path: path.to_string(),
            })
            .collect(),
        ..BuildrootImageSpec::default()
    });
    spec
}

#[test]
fn each_module_is_a_kernel_module_package_in_the_generated_tree() {
    let scratch = TempDir::new("gaia-kmod-packages");
    let root = scratch.join("workspace");
    let module = root.join("lemnos/kernel-modules/lemnos");
    fs::create_dir_all(&module).expect("module dir");
    fs::write(module.join("Kbuild"), "obj-m += lemnos.o\n").expect("Kbuild");
    let buildroot_dir = scratch.join("buildroot");
    let output_dir = scratch.join("output");
    fs::create_dir_all(buildroot_dir.join("package")).expect("buildroot");
    fs::create_dir_all(&output_dir).expect("output");
    let spec = spec_with(&root, &[("lemnos", "lemnos/kernel-modules/lemnos")]);

    let tree = materialize_buildroot_package_overrides(&spec, &buildroot_dir, &output_dir)
        .expect("materialized")
        .generated_external_tree
        .expect("a generated tree");
    assert_eq!(tree.package_count, 1);
    let package = tree.path.join("package/gaia-kmod-lemnos");
    let mk = fs::read_to_string(package.join("gaia-kmod-lemnos.mk")).expect(".mk");
    assert!(
        mk.contains(&format!("GAIA_KMOD_LEMNOS_SITE = {}\n", module.display())),
        "{mk}"
    );
    assert!(
        mk.contains("GAIA_KMOD_LEMNOS_SITE_METHOD = local\n"),
        "{mk}"
    );
    assert!(
        mk.contains("$(eval $(kernel-module))\n$(eval $(generic-package))\n"),
        "{mk}"
    );
    let config = fs::read_to_string(package.join("Config.in")).expect("Config.in");
    assert!(
        config.starts_with("config BR2_PACKAGE_GAIA_KMOD_LEMNOS\n"),
        "{config}"
    );
    assert!(config.contains("depends on BR2_LINUX_KERNEL"), "{config}");
    let top = fs::read_to_string(tree.path.join("Config.in")).expect("tree Config.in");
    assert!(top.contains("package/gaia-kmod-lemnos/Config.in"), "{top}");
}

#[test]
fn a_directory_without_kbuild_fails_before_buildroot_runs() {
    let scratch = TempDir::new("gaia-kmod-missing");
    let root = scratch.join("workspace");
    fs::create_dir_all(root.join("empty")).expect("dir");
    let output_dir = scratch.join("output");
    fs::create_dir_all(&output_dir).expect("output");
    let spec = spec_with(&root, &[("x", "empty")]);
    let error = materialize_buildroot_package_overrides(&spec, &scratch.join("br"), &output_dir)
        .err()
        .expect("refused");
    assert!(
        error.message.contains("no Kbuild or Makefile"),
        "{}",
        error.message
    );
}

#[test]
fn the_package_digest_follows_the_module_source() {
    let scratch = TempDir::new("gaia-kmod-digest");
    let root = scratch.join("workspace");
    let module = root.join("m");
    fs::create_dir_all(&module).expect("dir");
    fs::write(module.join("Kbuild"), "obj-m += m.o\n").expect("Kbuild");
    fs::write(module.join("m.c"), "int x;\n").expect("source");
    let spec = spec_with(&root, &[("m", "m")]);
    let digest = |spec: &ResolvedBuildSpec| generated_package_digests(spec).remove("gaia-kmod-m");
    let before = digest(&spec).expect("the module package has a digest");
    assert_eq!(Some(&before), digest(&spec).as_ref());
    fs::write(module.join("m.c"), "int y;\n").expect("edit");
    assert_ne!(Some(before), digest(&spec));
}
