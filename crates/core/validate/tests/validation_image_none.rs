//! `[image] kind = "none"`: what it accepts, and what it rejects because there
//! is no rootfs, kernel or image provider for the field to act on.

pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, validate_spec};
use support::write_temp_config;

/// A kind-none build with one path source and the given extra lines.
fn config(extra: &str) -> String {
    format!(
        r#"
build_name = "kind-none"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[reporting]
summary = true

[[sources]]
id = "files"
kind = "path"
path = "files"

[image]
kind = "none"

{extra}
"#
    )
}

/// The error codes a kind-none build raises.
fn error_codes(extra: &str) -> Vec<String> {
    let path = write_temp_config(&config(extra));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// The messages of the error diagnostics with `code`.
fn messages(extra: &str, code: &str) -> Vec<String> {
    let path = write_temp_config(&config(extra));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    validate_spec(&spec)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

const PACKAGE: &str = r#"
[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "pkg"
path = "$assembly.work/pkg"

[[image.assembly.files]]
tree = "pkg"
src = "${source.files.path}/a"
dest = "usr/bin/a"
"#;

#[test]
fn a_kind_none_build_with_only_sources_and_assembly_validates() {
    assert!(
        error_codes(PACKAGE).is_empty(),
        "{:?}",
        error_codes(PACKAGE)
    );
    assert!(error_codes("").is_empty());
}

#[test]
fn a_rootfs_feed_is_rejected_with_a_message_for_kind_none() {
    let codes = error_codes("[image.feed]\ninstall_entries = [\"x\"]\n");
    assert!(
        codes.iter().any(|code| code == "image_none_feed"),
        "{codes:?}"
    );
    let messages = messages(
        "[image.feed]\ninstall_entries = [\"x\"]\n",
        "image_none_feed",
    );
    assert!(
        messages[0].contains("kind = \"none\"") && messages[0].contains("[image.assembly]"),
        "{messages:?}"
    );
    let codes = error_codes("[image.feed]\nstage_files = [\"x\"]\n");
    assert!(
        codes.iter().any(|code| code == "image_none_feed"),
        "{codes:?}"
    );
}

#[test]
fn checks_and_hardware_are_rejected_for_kind_none() {
    let checks = error_codes("[image.checks]\nkernel_modules = false\n");
    assert!(
        checks.iter().any(|code| code == "image_none_checks"),
        "{checks:?}"
    );
    let hardware = error_codes("[[image.hardware]]\nid = \"camera\"\n");
    assert!(
        hardware
            .iter()
            .any(|code| code == "image_hardware_not_buildroot"),
        "{hardware:?}"
    );
}

#[test]
fn layers_are_rejected_for_kind_none() {
    let codes = error_codes("[image.layers]\norder = [\"base\"]\ndefault = \"base\"\n");
    assert!(
        codes
            .iter()
            .any(|code| code == "image_layers_kind_unsupported"),
        "{codes:?}"
    );
}

#[test]
fn a_provider_token_is_rejected_and_says_there_is_no_provider() {
    let extra = PACKAGE.replace("$assembly.work/pkg", "$provider.images/pkg");
    let codes = error_codes(&extra);
    assert!(
        codes
            .iter()
            .any(|code| code == "assembly_path_template_invalid"),
        "{codes:?}"
    );
    let messages = messages(&extra, "assembly_path_template_invalid");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("$provider.images")
                && message.contains("no image provider")),
        "{messages:?}"
    );
}

#[test]
fn a_checkpoint_anchored_at_the_image_needs_an_assembly() {
    let no_assembly = "[[checkpoints]]\nid = \"snap\"\nbackend = \"local\"\n\
                       anchor = \"image\"\nuse_policy = \"auto\"\nupload_policy = \"off\"\n";
    let codes = error_codes(no_assembly);
    assert!(
        codes
            .iter()
            .any(|code| code == "checkpoint_anchor_without_assembly"),
        "{codes:?}"
    );
    let with_assembly = format!("{PACKAGE}\n{no_assembly}");
    assert!(
        error_codes(&with_assembly).is_empty(),
        "{:?}",
        error_codes(&with_assembly)
    );
}
