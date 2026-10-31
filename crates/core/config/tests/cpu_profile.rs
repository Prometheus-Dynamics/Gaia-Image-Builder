//! `[cpu]`: a board layer's CPU profile reaches every Rust artifact built
//! for its architecture and the Buildroot image, and nothing else.

use gaia_config::try_resolve_config;
use gaia_spec::{ArtifactDefinition, ImageDefinition, ResolvedBuildSpec, RustCpuSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

const BOARD: &str = r#"
config = 3

[cpu]
profile = "cortex-a76"
"#;

const BUILD: &str = r#"
config = 3
build_name = "acme"
imports = ["board.toml"]

[workspace]
root_dir = "."

[[sources]]
id = "app"
kind = "path"
path = "app"

[[artifacts]]
id = "engine"
kind = "rust"
source = "app"
package = "engine"
output_path = "out/engine"
target = "aarch64-unknown-linux-gnu"

[[artifacts]]
id = "generic"
kind = "rust"
source = "app"
package = "generic"
output_path = "out/generic"
target = "aarch64-unknown-linux-gnu"
cpu = false

[[artifacts]]
id = "host-tool"
kind = "rust"
source = "app"
package = "host-tool"
output_path = "out/host-tool"
target = "x86_64-unknown-linux-gnu"

[[artifacts]]
id = "native"
kind = "rust"
source = "app"
package = "native"
output_path = "out/native"

[image]
kind = "buildroot"
defconfig = "${self}/board_defconfig"
config_overrides = [["BR2_cortex_a76", "n"]]
"#;

fn resolve(root: &Path, board: &str, build: &str) -> Result<ResolvedBuildSpec, String> {
    fs::create_dir_all(root.join("app")).expect("app");
    fs::write(root.join("board_defconfig"), "BR2_aarch64=y\n").expect("defconfig");
    fs::write(root.join("board.toml"), board).expect("board");
    fs::write(root.join("build.toml"), build).expect("build");
    try_resolve_config(root.join("build.toml").to_str().expect("utf-8"))
        .map_err(|error| error.to_string())
}

fn cpu(spec: &ResolvedBuildSpec, id: &str) -> Option<RustCpuSpec> {
    let artifact = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("artifact");
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.cpu.clone(),
        _ => panic!("rust artifact"),
    }
}

#[test]
fn the_profile_reaches_rust_artifacts_of_its_architecture_and_buildroot() {
    let scratch = TempDir::new("gaia-config-cpu");
    let spec = resolve(scratch.path(), BOARD, BUILD).expect("resolves");

    let engine = cpu(&spec, "engine").expect("aarch64 artifact gets the profile");
    assert_eq!(engine.profile, "cortex-a76");
    assert_eq!(engine.rustflags, ["-C", "target-cpu=cortex-a76"]);
    assert_eq!(engine.c_flags, ["-mcpu=cortex-a76"]);
    assert_eq!(cpu(&spec, "generic"), None, "cpu = false opts out");
    assert_eq!(cpu(&spec, "host-tool"), None, "another architecture");
    assert_eq!(cpu(&spec, "native"), None, "no target: a host build");

    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        panic!("buildroot image");
    };
    // The profile's symbol comes first, so the image's own override wins.
    assert_eq!(
        buildroot.config_overrides,
        [
            ("BR2_cortex_a76".to_string(), "y".to_string()),
            ("BR2_cortex_a76".to_string(), "n".to_string()),
        ]
    );
}

#[test]
fn a_later_layer_changes_the_profile_and_buildroot_can_be_left_alone() {
    let scratch = TempDir::new("gaia-config-cpu-layers");
    let build = format!("{BUILD}\n[cpu]\nprofile = \"cortex-a55\"\nbuildroot = false\n")
        .replace("config_overrides = [[\"BR2_cortex_a76\", \"n\"]]\n", "");
    let spec = resolve(scratch.path(), BOARD, &build).expect("resolves");
    assert_eq!(
        cpu(&spec, "engine").expect("profile").rustflags,
        ["-C", "target-cpu=cortex-a55"]
    );
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        panic!("buildroot image");
    };
    assert!(buildroot.config_overrides.is_empty());
}

#[test]
fn an_unknown_profile_lists_the_known_ones() {
    let scratch = TempDir::new("gaia-config-cpu-unknown");
    let error = resolve(
        scratch.path(),
        "config = 3\n\n[cpu]\nprofile = \"cortex-a99\"\n",
        BUILD,
    )
    .expect_err("unknown profile");
    assert!(
        error.contains("[cpu] profile 'cortex-a99' is not one Gaia knows")
            && error.contains("'cortex-a76' (CM5 (BCM2712))")
            && error.contains("'xuantie-c908'"),
        "{error}"
    );
}

#[test]
fn a_build_without_a_profile_serializes_as_before() {
    let scratch = TempDir::new("gaia-config-cpu-none");
    let spec = resolve(scratch.path(), "config = 3\n", BUILD).expect("resolves");
    assert_eq!(cpu(&spec, "engine"), None);
    assert!(!gaia_spec::canonical_json(&spec.artifacts).contains("\"cpu\""));
}
