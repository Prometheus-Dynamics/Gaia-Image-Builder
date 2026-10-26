//! `[[toolchains]]`, the `url` form of an archive source, and the device table
//! of a package: how the config compiles them into the spec.

pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{
    ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec, ToolchainKindSpec,
};
use support::write_temp_config;

const SHA: &str = "3c9f1ab2f8c7ce1d6e4e4f8a0b5c9d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c";

fn compile(text: &str) -> ResolvedBuildSpec {
    let path = write_temp_config(text);
    resolve_config(path.to_str().expect("temp path utf-8"))
}

fn artifact<'a>(spec: &'a ResolvedBuildSpec, id: &str) -> &'a ArtifactSpec {
    spec.artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("artifact")
}

#[test]
fn a_toolchain_compiles_with_its_root_sysroot_and_rust_target() {
    let spec = compile(&format!(
        r#"
build_name = "toolchain-compile"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "sdk"
kind = "archive"
url = "https://example.invalid/sdk-toolchain.tar.gz"
sha256 = "{SHA}"
strip_components = 1

[[sources]]
id = "sysroot-tree"
kind = "path"
path = "sysroot"

[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
source = "sdk"
relocate = true
triple = "aarch64-buildroot-linux-gnu"
sysroot_source = "sysroot-tree"
rust_target = "aarch64-unknown-linux-gnu"
page_size = 16384

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
toolchain = "systemcore"
output_path = "out/app"
"#
    ));

    assert_eq!(spec.toolchains.len(), 1);
    let toolchain = &spec.toolchains[0];
    assert_eq!(toolchain.id, "systemcore");
    assert_eq!(toolchain.kind, ToolchainKindSpec::BuildrootSdk);
    assert_eq!(
        toolchain.root_source.as_ref().map(|id| id.as_str()),
        Some("sdk")
    );
    assert_eq!(toolchain.root_path, None);
    assert!(toolchain.relocate);
    assert_eq!(toolchain.prefix, None, "the default prefix is the triple's");
    assert_eq!(
        toolchain.sysroot_source.as_ref().map(|id| id.as_str()),
        Some("sysroot-tree")
    );
    assert_eq!(toolchain.page_size, Some(16384));
    assert_eq!(
        toolchain.rust_target.as_deref(),
        Some("aarch64-unknown-linux-gnu")
    );

    let app = artifact(&spec, "app");
    assert_eq!(app.toolchain.as_deref(), Some("systemcore"));
    // The artifact reads the SDK's tree and its sysroot's tree, so it depends
    // on both sources.
    let extra: Vec<&str> = app.extra_sources.iter().map(|id| id.as_str()).collect();
    assert_eq!(extra, ["sdk", "sysroot-tree"]);
    assert!(matches!(app.definition, ArtifactDefinition::Rust(_)));
}

#[test]
fn an_archive_with_a_url_is_pinned_by_its_checksum_and_has_no_path() {
    let spec = compile(&format!(
        r#"
build_name = "archive-url"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "bundle"
kind = "archive"
url = "https://example.invalid/bundle.tar.xz"
sha256 = "{SHA}"
strip_components = 1
"#
    ));
    let source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "bundle")
        .expect("bundle");
    let SourceDefinition::Archive(archive) = &source.definition else {
        panic!("archive source");
    };
    assert!(archive.path.is_empty());
    assert_eq!(
        archive.url.as_deref(),
        Some("https://example.invalid/bundle.tar.xz")
    );
    assert_eq!(archive.sha256.as_deref(), Some(SHA));
    assert_eq!(archive.strip_components, 1);
    assert_eq!(archive.refresh_policy, SourceRefreshPolicySpec::Auto);
    assert_eq!(archive.pin_policy, SourcePinPolicySpec::Locked);
}

#[test]
fn a_path_archive_keeps_its_path_and_floating_pin() {
    let spec = compile(
        r#"
build_name = "archive-path"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "seed"
kind = "archive"
path = "@assets/seed.tar"
strip_components = 0
"#,
    );
    let SourceDefinition::Archive(archive) = &spec.sources[0].definition else {
        panic!("archive source");
    };
    assert_eq!(archive.path, "@assets/seed.tar");
    assert_eq!(archive.url, None);
    assert_eq!(archive.sha256, None);
    assert_eq!(archive.pin_policy, SourcePinPolicySpec::Floating);
}

#[test]
fn a_package_device_table_compiles_with_its_defaults() {
    let spec = compile(
        r#"
build_name = "device-table"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "files"
kind = "path"
path = "files"

[image]
kind = "none"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
out_dir = "${workspace.out_dir}"

[[image.assembly.trees]]
id = "pkg-root"
path = "$assembly.work/pkg-root"

[[image.assembly.packages]]
id = "tool"
tree = "pkg-root"
dest = "$assembly.out/"

[image.assembly.packages.control]
Package = "tool"
Version = "1.0.0"
Architecture = "aarch64"
Maintainer = "Test <test@example.invalid>"
Description = "A tool."

[image.assembly.packages.device]
toolchain = "systemcore"
max_glibc = "2.17"
"#,
    );
    let assembly = spec.image.assembly.as_ref().expect("assembly");
    let device = assembly.packages[0].device.as_ref().expect("device");
    assert_eq!(device.toolchain.as_deref(), Some("systemcore"));
    assert_eq!(device.sysroot, None);
    assert_eq!(device.max_glibc.as_deref(), Some("2.17"));
    assert_eq!(
        device.min_page_size, None,
        "resolved from the toolchain at check time"
    );
    assert!(device.check_needed, "DT_NEEDED is checked by default");
}

#[test]
fn a_package_without_a_device_table_has_no_device_checks() {
    let spec = compile(
        r#"
build_name = "no-device"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "none"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
out_dir = "${workspace.out_dir}"

[[image.assembly.trees]]
id = "pkg-root"
path = "$assembly.work/pkg-root"

[[image.assembly.packages]]
id = "tool"
tree = "pkg-root"
dest = "$assembly.out/"

[image.assembly.packages.control]
Package = "tool"
Version = "1.0.0"
Architecture = "all"
Maintainer = "Test <test@example.invalid>"
Description = "A tool."
"#,
    );
    let assembly = spec.image.assembly.as_ref().expect("assembly");
    assert!(assembly.packages[0].device.is_none());
}
