//! Config v3 files resolve to the same spec as their v2 spelling, layers of
//! both versions mix, and v3's errors point at the line.

pub mod support;

use gaia_config::{migrate_builds, try_resolve_config};
use gaia_spec::ResolvedBuildSpec;
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

const V2_WORKSPACE: &str = r#"
[workspace]
root_dir = "."
build_dir = "build"
out_dir = "gaia/output/${build.name}"

[[workspace.named_paths]]
alias = "assets"
path = "gaia/assets"
kind = "host"

[providers.buildroot]
work_dir = "ram"
download_dir = "/srv/dl"

[providers.buildroot.ccache]
enabled = true
max_size = "50G"

[providers.buildroot.package_cache]
enabled = true

[providers.rust]
sccache = true
"#;

const V3_WORKSPACE: &str = r#"
config = 3

[workspace]
root_dir = "."
build_dir = "build"

[[workspace.named_paths]]
alias = "assets"
path = "gaia/assets"
kind = "host"

[outputs]
dir = "gaia/output/${build.name}"

[providers.buildroot]
work_dir = "ram"

[cache]
downloads = "/srv/dl"
ccache = { enabled = true, max_size = "50G" }
packages = { enabled = true }
sccache = true
"#;

const V2_BUILD: &str = r#"
build_name = "acme"
version = "1.2.3"
extends = "workspace.toml"

[image]
kind = "buildroot"
defconfig = "@self/board_defconfig"

[image.output]
collect_dir = "${workspace.out_dir}/images"
emit_report = true

[image.assembly]
work_dir = "$provider.images/work"

[[image.assembly.trees]]
id = "rootfs"
path = "$assembly.work/rootfs"

[[image.assembly.files]]
tree = "rootfs"
src = "@assets/motd"
dest = "etc/motd"

[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"
"#;

const V3_BUILD: &str = r#"
config = 3
build_name = "acme"
version = "1.2.3"
extends = "workspace.toml"

[image]
kind = "buildroot"
defconfig = "${self}/board_defconfig"

[outputs]
images_dir = "${outputs.dir}/images"
report = true

[image.assembly]
work_dir = "${provider.images}/work"

[[image.assembly.trees]]
id = "rootfs"
path = "${assembly.work}/rootfs"

[[image.assembly.files]]
tree = "rootfs"
src = "${assets}/motd"
dest = "etc/motd"

[[image.assembly.transforms]]
kind = "verity"
src = "${provider.images}/rootfs.erofs"
dest = "${assembly.work}/rootfs.verity.erofs"
"#;

/// The build at `root/build.toml`, extending `root/workspace.toml`, resolved
/// with the given texts for both files.
fn resolve(root: &Path, workspace: &str, build: &str) -> ResolvedBuildSpec {
    fs::create_dir_all(root.join("gaia/assets")).expect("assets dir");
    fs::write(root.join("gaia/assets/motd"), "hi\n").expect("motd");
    fs::write(root.join("board_defconfig"), "BR2_aarch64=y\n").expect("defconfig");
    fs::write(root.join("workspace.toml"), workspace).expect("workspace");
    fs::write(root.join("build.toml"), build).expect("build");
    try_resolve_config(root.join("build.toml").to_str().expect("utf-8"))
        .unwrap_or_else(|error| panic!("resolves: {error}"))
}

#[test]
fn v3_files_resolve_to_the_spec_of_their_v2_spelling() {
    let scratch = TempDir::new("gaia-config-v3");
    let root = scratch.join("build");

    let mut v2 = resolve(&root, V2_WORKSPACE, V2_BUILD);
    let mut v3 = resolve(&root, V3_WORKSPACE, V3_BUILD);
    assert_eq!(v2.metadata.config_v2_files.len(), 2);
    assert!(v3.metadata.config_v2_files.is_empty());
    v2.metadata.config_v2_files.clear();
    v3.metadata.config_v2_files.clear();
    assert_eq!(v2, v3);

    // A v3 build may extend a v2 layer, and the other way round.
    let mixed = resolve(&root, V2_WORKSPACE, V3_BUILD);
    assert_eq!(mixed.metadata.config_v2_files.len(), 1);
    assert_eq!(mixed.image, v2.image);
    assert_eq!(mixed.workspace, v2.workspace);
    let mixed = resolve(&root, V3_WORKSPACE, V2_BUILD);
    assert_eq!(mixed.policy, v2.policy);
    assert_eq!(mixed.image, v2.image);
}

#[test]
fn an_alias_no_layer_declares_is_an_error_at_its_line() {
    let scratch = TempDir::new("gaia-config-v3-alias");
    let root = scratch.join("build");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("workspace.toml"), V3_WORKSPACE).expect("workspace");
    fs::write(
        root.join("build.toml"),
        V3_BUILD.replace("${assets}/motd", "${asets}/motd"),
    )
    .expect("build");

    let error = try_resolve_config(root.join("build.toml").to_str().expect("utf-8"))
        .expect_err("unknown alias")
        .to_string();
    assert!(error.contains("build.toml:24:1"), "{error}");
    assert!(
        error.contains("`${asets}` is neither a token nor a `[[workspace.named_paths]]` alias"),
        "{error}"
    );
    assert!(error.contains("${assets}"), "{error}");
}

#[test]
fn migrate_rewrites_a_build_and_its_layers_to_the_same_spec() {
    let scratch = TempDir::new("gaia-config-v3-migrate");
    let root = scratch.join("build");
    let before = resolve(&root, V2_WORKSPACE, V2_BUILD);
    let build = root.join("build.toml").display().to_string();

    let check = migrate_builds(std::slice::from_ref(&build), true).expect("check");
    assert_eq!(check.v2_files().count(), 2);
    assert!(check.migrated.is_empty());
    assert!(
        fs::read_to_string(root.join("build.toml"))
            .unwrap()
            .contains("@self/")
    );

    let report = migrate_builds(std::slice::from_ref(&build), false).expect("migrates");
    assert_eq!(report.migrated.len(), 2);
    assert_eq!(report.compared, [format!("build '{build}'")]);
    let migrated = fs::read_to_string(root.join("build.toml")).unwrap();
    assert!(
        migrated.starts_with("\nconfig = 3\nbuild_name"),
        "{migrated}"
    );
    assert!(
        migrated.contains("defconfig = \"${self}/board_defconfig\""),
        "{migrated}"
    );
    let workspace = fs::read_to_string(root.join("workspace.toml")).unwrap();
    assert!(workspace.contains("[cache.ccache]"), "{workspace}");

    let mut after = try_resolve_config(&build).expect("resolves");
    assert!(after.metadata.config_v2_files.is_empty());
    let mut before = before;
    before.metadata.config_v2_files.clear();
    after.metadata.config_v2_files.clear();
    assert_eq!(before, after);
    assert_eq!(
        migrate_builds(&[build], true).unwrap().v2_files().count(),
        0
    );
}

#[test]
fn a_migration_that_would_change_the_build_is_undone() {
    let scratch = TempDir::new("gaia-config-v3-undo");
    let root = scratch.join("build");
    // v2 lets the build's image export dir win over the workspace's,
    // whichever layer sets it; v3 has one key, which the extending layer
    // sets last.
    let workspace = format!("{V2_WORKSPACE}\n[image.output]\nexport_dir = \"/srv/image\"\n");
    let build = V2_BUILD.replace(
        "extends = \"workspace.toml\"\n",
        "extends = \"workspace.toml\"\n\n[workspace]\nexport_dir = \"/srv/shared\"\n",
    );
    let build = build.replace("[image.output]\n", "[image.output]\n# no export dir here\n");
    resolve(&root, &workspace, &build);
    let path = root.join("build.toml").display().to_string();

    let error = migrate_builds(std::slice::from_ref(&path), false)
        .expect_err("export dir changes")
        .to_string();
    assert!(
        error.contains(
            "resolves differently after migrating (`export.image_dir` was \"/srv/image\", is \"/srv/shared\")"
        ),
        "{error}"
    );
    assert_eq!(fs::read_to_string(root.join("build.toml")).unwrap(), build);
    assert_eq!(
        fs::read_to_string(root.join("workspace.toml")).unwrap(),
        workspace
    );
}
