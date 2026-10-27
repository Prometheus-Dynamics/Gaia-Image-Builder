//! Patch entries (an `[[artifacts]]`, `[[install]]`, `[[stage.files]]` or
//! `[[stage.services]]` entry without its declaring key patches the earlier
//! entry with the same id), the notes that record them, and the image kind
//! switch.
//!
//! Each build here is an entrypoint that extends `base.toml` (the shared
//! declarations, merged first) and imports `layer.toml` (the patches). The
//! entrypoint's own declarations would merge last, after its imports.

pub mod support;

use gaia_config::{ResolveOptions, try_resolve_config, try_resolve_config_with_options};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ImageDefinition, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::path::{Path, PathBuf};
use support::write_temp_config_at;

const ENTRY: &str = r#"
build_name = "patched"
extends = "base.toml"
imports = ["layer.toml"]

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"
"#;

/// The shared declarations: one rust artifact and its install, under
/// `image.kind = "none"`.
const BASE: &str = r#"
[image]
kind = "none"

[[artifacts]]
id = "helios-api"
kind = "rust"
package = "helios-app"
target_name = "helios-api"
source = "app"
profile = "debug"
output_path = "${workspace.out_dir}/artifacts/helios-api"

[[install]]
id = "install-helios-api"
artifact = "helios-api"
dest = "/usr/bin/helios-api"
"#;

/// Writes `base.toml` and `layer.toml` next to the entrypoint, which is
/// written too and returned.
fn build_with(scratch: &TempDir, base: &str, layer: &str) -> PathBuf {
    write_temp_config_at(&scratch.join("base.toml"), base);
    write_temp_config_at(&scratch.join("layer.toml"), layer);
    let entry = scratch.join("build.toml");
    write_temp_config_at(&entry, ENTRY);
    entry
}

fn resolve(entry: &Path) -> ResolvedBuildSpec {
    try_resolve_config(entry.to_str().expect("utf-8 path")).expect("the build resolves")
}

fn resolve_error(entry: &Path) -> String {
    try_resolve_config(entry.to_str().expect("utf-8 path"))
        .expect_err("the build is refused")
        .to_string()
}

/// The 1-based line of the first line of `text` that contains `needle`.
fn line_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|line| line.contains(needle))
        .expect("the needle is in the text")
        + 1
}

fn artifact<'a>(spec: &'a ResolvedBuildSpec, id: &str) -> &'a ArtifactSpec {
    spec.artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("the artifact")
}

fn rust_package(spec: &ResolvedBuildSpec, id: &str) -> String {
    match &artifact(spec, id).definition {
        ArtifactDefinition::Rust(rust) => rust.package.clone(),
        other => panic!("expected a rust artifact, got {other:?}"),
    }
}

#[test]
fn a_patch_replaces_the_keys_it_sets_and_keeps_the_rest() {
    let scratch = TempDir::new("gaia-config-patch-field");
    let entry = build_with(
        &scratch,
        BASE,
        "[[artifacts]]\nid = \"helios-api\"\ntoolchain = \"sdk\"\nprofile = \"release\"\n",
    );

    let spec = resolve(&entry);

    let helios = artifact(&spec, "helios-api");
    assert_eq!(helios.toolchain.as_deref(), Some("sdk"));
    assert!(helios.build_mode.is_some(), "profile is set by the patch");
    // Kept from the declaration: the package and the output.
    assert_eq!(rust_package(&spec, "helios-api"), "helios-app");
    assert!(
        helios.output.path.ends_with("artifacts/helios-api"),
        "{}",
        helios.output.path
    );
}

#[test]
fn a_patch_without_an_earlier_entry_is_an_error_naming_the_layer_line() {
    let scratch = TempDir::new("gaia-config-patch-missing");
    let layer = "# a comment\n[[artifacts]]\nid = \"ghost\"\nprofile = \"release\"\n";
    let entry = build_with(&scratch, BASE, layer);

    let error = resolve_error(&entry);

    let expected = format!(
        "artifact patch 'ghost' in layer.toml:{} has no earlier [[artifacts]] entry to patch; add kind to declare it",
        line_of(layer, "[[artifacts]]")
    );
    assert_eq!(error, expected);
}

#[test]
fn a_patch_that_sets_only_its_id_is_an_error() {
    let scratch = TempDir::new("gaia-config-patch-id-only");
    let entry = build_with(&scratch, BASE, "[[artifacts]]\nid = \"helios-api\"\n");

    let error = resolve_error(&entry);

    assert!(
        error.contains("artifact patch 'helios-api' in layer.toml:1 sets no field to change"),
        "{error}"
    );
}

#[test]
fn the_notes_say_which_layer_set_each_patched_field() {
    let scratch = TempDir::new("gaia-config-patch-notes");
    let layer = "[[artifacts]]\nid = \"helios-api\"\ntoolchain = \"sdk\"\ntarget = \"aarch64-unknown-linux-gnu\"\n";
    let entry = build_with(&scratch, BASE, layer);

    let spec = resolve(&entry);

    let note = "artifact 'helios-api': toolchain, target set by layer.toml:1";
    assert_eq!(spec.metadata.config_notes, vec![note.to_string()]);
}

#[test]
fn a_patch_replaces_a_table_value_as_a_whole() {
    let scratch = TempDir::new("gaia-config-patch-table");
    let base = format!("{BASE}\n[artifacts.execution.docker]\nimage = \"builder:1\"\n");
    let entry = build_with(
        &scratch,
        &base,
        "[[artifacts]]\nid = \"helios-api\"\n[artifacts.execution]\nbackend = \"host\"\n",
    );

    let spec = resolve(&entry);

    let execution = artifact(&spec, "helios-api")
        .execution
        .as_ref()
        .expect("execution is set by the patch");
    let text = format!("{execution:?}");
    assert!(
        !text.contains("builder:1"),
        "the docker table was not replaced: {text}"
    );
}

#[test]
fn install_and_stage_patches_retarget_their_entries() {
    let scratch = TempDir::new("gaia-config-patch-stage");
    let base = format!(
        "{BASE}\n[[stage.files]]\nid = \"motd\"\nsrc = \"assets/motd\"\ndest = \"/etc/motd\"\nmode = 0o644\n\n[[stage.services]]\nid = \"helios\"\nname = \"helios\"\nunit_path = \"helios.service\"\n"
    );
    let layer = "[[install]]\nid = \"install-helios-api\"\ndest = \"/usr/local/bin/helios-api\"\n\n[[stage.files]]\nid = \"motd\"\nmode = 0o600\n\n[[stage.services]]\nid = \"helios\"\nunit_path = \"helios-2.service\"\n";
    let entry = build_with(&scratch, &base, layer);

    let spec = resolve(&entry);

    let install = spec
        .install
        .entries
        .iter()
        .find(|install| install.id.as_str() == "install-helios-api")
        .expect("the install");
    assert_eq!(install.dest, "/usr/local/bin/helios-api");
    assert_eq!(
        install.artifact.id.as_str(),
        "helios-api",
        "kept from the declaration"
    );
    let notes = spec.metadata.config_notes.join("\n");
    assert!(
        notes.contains("install 'install-helios-api': dest set by layer.toml:1"),
        "{notes}"
    );
    assert!(
        notes.contains("stage file 'motd': mode set by layer.toml:"),
        "{notes}"
    );
    assert!(
        notes.contains("stage service 'helios': unit_path set by layer.toml:"),
        "{notes}"
    );
}

#[test]
fn a_stage_file_content_entry_renders_config_tokens_and_patches_switch_src_and_content() {
    let scratch = TempDir::new("gaia-config-stage-content");
    let base = format!(
        "version = \"2026.2.0-dev.7\"\n{BASE}\n[[stage.files]]\nid = \"os-release\"\ncontent = \"VERSION_ID=\\\"${{build.version}}\\\"\\n\"\ndest = \"/etc/os-release\"\n\n[[stage.files]]\nid = \"motd\"\nsrc = \"assets/motd\"\ndest = \"/etc/motd\"\n"
    );
    let layer = "[[stage.files]]\nid = \"motd\"\ncontent = \"hello\\n\"\n";
    let entry = build_with(&scratch, &base, layer);

    let spec = resolve(&entry);

    let file = |id: &str| {
        spec.stage
            .files
            .iter()
            .find(|file| file.id.as_str() == id)
            .expect("the stage file")
    };
    assert_eq!(
        file("os-release").content.as_deref(),
        Some("VERSION_ID=\"2026.2.0-dev.7\"\n")
    );
    assert_eq!(file("os-release").src, "");
    assert_eq!(file("motd").content.as_deref(), Some("hello\n"));
    assert_eq!(file("motd").src, "", "content replaces the declared src");
}

#[test]
fn a_stage_patch_without_an_earlier_entry_names_its_declaring_key() {
    let scratch = TempDir::new("gaia-config-patch-stage-missing");
    let entry = build_with(
        &scratch,
        BASE,
        "[[stage.files]]\nid = \"motd\"\nmode = 0o600\n",
    );

    let error = resolve_error(&entry);

    assert_eq!(
        error,
        "stage file patch 'motd' in layer.toml:1 has no earlier [[stage.files]] entry to patch; add src or content to declare it"
    );
}

#[test]
fn an_entry_with_its_declaring_key_is_a_full_declaration_not_a_patch() {
    let scratch = TempDir::new("gaia-config-patch-declaration");
    let layer = "[[artifacts]]\nid = \"helios-cli\"\nkind = \"rust\"\npackage = \"helios-app\"\ntarget_name = \"helios-cli\"\nsource = \"app\"\noutput_path = \"out/helios-cli\"\n";
    let entry = build_with(&scratch, BASE, layer);

    let spec = resolve(&entry);

    assert_eq!(rust_package(&spec, "helios-cli"), "helios-app");
    assert!(
        spec.metadata.config_notes.is_empty(),
        "{:?}",
        spec.metadata.config_notes
    );
}

#[test]
fn a_patch_of_an_entry_declared_earlier_in_the_same_file_applies_after_it() {
    let scratch = TempDir::new("gaia-config-patch-same-file");
    let base = format!("{BASE}\n[[artifacts]]\nid = \"helios-api\"\ntoolchain = \"sdk\"\n");
    let entry = build_with(&scratch, &base, "# nothing to patch here\n");

    let spec = resolve(&entry);

    assert_eq!(
        artifact(&spec, "helios-api").toolchain.as_deref(),
        Some("sdk")
    );
}

#[test]
fn an_empty_toolchain_after_interpolation_means_the_host() {
    let scratch = TempDir::new("gaia-config-toolchain-empty");
    let base = format!(
        "{BASE}\n[inputs.toolchain]\nkind = \"string\"\ndefault = \"\"\n\n[[artifacts]]\nid = \"helios-api\"\ntoolchain = \"${{input.toolchain}}\"\n"
    );
    let entry = build_with(&scratch, &base, "# nothing to patch here\n");

    let spec = resolve(&entry);
    assert_eq!(artifact(&spec, "helios-api").toolchain, None);

    let with_sdk = try_resolve_config_with_options(
        entry.to_str().expect("utf-8 path"),
        &ResolveOptions {
            explicit_overrides: vec![("input.toolchain".into(), "systemcore".into())],
            ..ResolveOptions::default()
        },
    )
    .expect("the build resolves with the input set");
    assert_eq!(
        artifact(&with_sdk, "helios-api").toolchain.as_deref(),
        Some("systemcore")
    );
}

const BUILDROOT_BASE: &str = r#"
[image]
kind = "buildroot"
defconfig = "base_defconfig"
external_tree_mode = "disabled"

[image.feed]
install_entries = ["install-helios-api"]

[image.output]
archive_name = "switch.tar"
collect_dir = "out/images"

[[install]]
id = "install-helios-api"
artifact = "helios-api"
dest = "/usr/bin/helios-api"

[[artifacts]]
id = "helios-api"
kind = "go"
package = "./cmd/helios"
output_path = "out/helios-api"
"#;

#[test]
fn a_layer_setting_another_kind_replaces_the_kind_specific_fields() {
    let scratch = TempDir::new("gaia-config-kind-switch");
    let entry = build_with(
        &scratch,
        BUILDROOT_BASE,
        "[image]\nkind = \"none\"\n\n[image.assembly]\nwork_dir = \"build/assembly\"\n",
    );

    let spec = resolve(&entry);

    assert!(matches!(spec.image.definition, ImageDefinition::None));
    assert!(
        spec.image.feed.install_entries.is_empty(),
        "the feed is replaced"
    );
    assert_eq!(
        spec.image.output.archive_name.as_deref(),
        Some("switch.tar"),
        "[image.output] keeps merging"
    );
    assert!(
        spec.image.assembly.is_some(),
        "[image.assembly] keeps merging"
    );
    assert_eq!(
        spec.metadata.config_notes,
        vec!["image kind none (layer.toml) replaces buildroot fields: defconfig, external_tree_mode, feed".to_string()]
    );
}

#[test]
fn a_layer_setting_the_same_kind_merges_into_the_image() {
    let scratch = TempDir::new("gaia-config-kind-same");
    let entry = build_with(
        &scratch,
        BUILDROOT_BASE,
        "[image]\nkind = \"buildroot\"\nconfig_overrides = [[\"BR2_TARGET_ROOTFS_EXT2\", \"n\"]]\n",
    );

    let spec = resolve(&entry);

    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            assert_eq!(buildroot.defconfig.as_deref(), Some("base_defconfig"));
        }
        other => panic!("expected buildroot, got {other:?}"),
    }
    assert_eq!(spec.image.feed.install_entries.len(), 1, "the feed merges");
    assert!(
        spec.metadata.config_notes.is_empty(),
        "{:?}",
        spec.metadata.config_notes
    );
}
