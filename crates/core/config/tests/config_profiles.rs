//! The two-profile example (`examples/profiles`): one build file, resolved
//! once per `build.target`. `raze` is a Buildroot image; `systemcore` is an
//! ipk package with no image, built with a toolchain its layer declares.

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{ArtifactDefinition, ImageDefinition, ResolvedBuildSpec};

fn example() -> String {
    format!(
        "{}/../../../examples/profiles/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn resolve_target(target: &str) -> ResolvedBuildSpec {
    try_resolve_config_with_options(
        &example(),
        &ResolveOptions {
            explicit_overrides: vec![("build.target".into(), target.into())],
            ..ResolveOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("{target} resolves: {error}"))
}

#[test]
fn the_raze_profile_is_a_buildroot_image_whose_artifacts_build_for_the_host() {
    let spec = resolve_target("raze");

    assert!(matches!(
        spec.image.definition,
        ImageDefinition::Buildroot(_)
    ));
    assert_eq!(
        spec.image.feed.install_entries.len(),
        2,
        "both installs feed the image"
    );
    assert!(
        spec.toolchains.is_empty(),
        "the systemcore layer is not selected"
    );
    for artifact in &spec.artifacts {
        assert_eq!(
            artifact.toolchain,
            None,
            "{} builds for the host",
            artifact.id.as_str()
        );
    }
    assert!(
        spec.metadata.config_notes.is_empty(),
        "{:?}",
        spec.metadata.config_notes
    );
}

#[test]
fn the_systemcore_profile_is_an_ipk_package_built_with_its_sdk() {
    let spec = resolve_target("systemcore");

    assert!(matches!(spec.image.definition, ImageDefinition::None));
    assert!(spec.image.feed.install_entries.is_empty());
    assert_eq!(spec.toolchains.len(), 1);
    assert_eq!(spec.toolchains[0].id.as_str(), "systemcore");
    for id in ["helios-api", "helios-cli"] {
        let artifact = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id.as_str() == id)
            .expect("the shared artifact");
        assert_eq!(artifact.toolchain.as_deref(), Some("systemcore"), "{id}");
        assert!(matches!(artifact.definition, ArtifactDefinition::Rust(_)));
    }
    let cli = spec
        .install
        .entries
        .iter()
        .find(|install| install.id.as_str() == "install-helios-cli")
        .expect("the retargeted install");
    assert_eq!(cli.dest, "/usr/local/bin/helios-cli");

    let assembly = spec.image.assembly.as_ref().expect("the ipk assembly");
    let package = assembly.packages.first().expect("one package");
    assert_eq!(package.id, "helios-systemcore");
    assert_eq!(package.format, "ipk");

    let notes = &spec.metadata.config_notes;
    for expected in [
        "artifact 'helios-api': toolchain set by profiles/systemcore.toml:",
        "artifact 'helios-cli': toolchain set by profiles/systemcore.toml:",
        "install 'install-helios-cli': dest set by profiles/systemcore.toml:",
        "image kind none (profiles/systemcore.toml) replaces buildroot fields: source, defconfig_path, external_tree_mode, expected_images, feed",
    ] {
        assert!(
            notes.iter().any(|note| note.starts_with(expected)),
            "missing {expected:?} in {notes:?}"
        );
    }
}
