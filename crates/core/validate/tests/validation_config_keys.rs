pub mod support;

use gaia_config::{ConfigError, try_resolve_config};
use gaia_validate::validate_spec;
use support::write_temp_config;

const BASE: &str = r#"
[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#;

#[test]
fn gaia_version_is_checked_before_any_other_config_validation() {
    // The tuple form of named_paths is a shape error, and `kind = "future"`
    // would not deserialize: neither may be reported before the version.
    let path = write_temp_config(
        r#"
gaia_version = ">=99.0.0"
build_name = "from-the-future"

[workspace]
named_paths = [["assets", "assets"]]

[image]
kind = "future"
"#,
    );

    let error = try_resolve_config(path.to_str().expect("utf-8 path"))
        .expect_err("a newer gaia_version must fail");

    let ConfigError::GaiaVersionUnsupported {
        required,
        installed,
        ..
    } = &error
    else {
        panic!("unexpected error: {error}");
    };
    assert_eq!(required, ">=99.0.0");
    assert_eq!(installed, gaia_config::GAIA_VERSION);
    assert!(error.to_string().contains(&format!(
        "this build requires gaia >=99.0.0, but gaia {} is installed",
        gaia_config::GAIA_VERSION
    )));
    let _ = std::fs::remove_file(path);
}

#[test]
fn unknown_keys_fail_validation() {
    let path = write_temp_config(&format!(
        r#"
gaia_version = ">=2.0.0"
build_name = "unknown-keys"
build_command = "make all"
{BASE}
[providers.buildroot]
override_check = "warn"
shared_outptu = true
"#
    ));

    let spec = try_resolve_config(path.to_str().expect("utf-8 path")).expect("config resolves");
    let report = validate_spec(&spec);

    assert_eq!(
        spec.policy.providers.buildroot.override_check,
        gaia_spec::BuildrootOverrideCheckSpec::Warn
    );
    let unknown = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "config_unknown_key")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    assert!(unknown[0].starts_with("unknown key 'build_command' in '"));
    assert!(unknown[1].starts_with("unknown key 'providers.buildroot.shared_outptu' in '"));
    assert!(
        report
            .errors
            .iter()
            .all(|error| error.contains("unknown key '")),
        "{:?}",
        report.errors
    );
    assert_eq!(report.errors.len(), 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn known_keys_produce_no_unknown_key_warnings() {
    let path = write_temp_config(&format!("build_name = \"known-keys\"\n{BASE}"));

    let spec = try_resolve_config(path.to_str().expect("utf-8 path")).expect("config resolves");

    assert!(spec.metadata.config_warnings.is_empty());
    assert_eq!(
        spec.policy.providers.buildroot.override_check,
        gaia_spec::BuildrootOverrideCheckSpec::Error
    );
    let _ = std::fs::remove_file(path);
}

/// A build file importing `layer.toml` with `layer` as its contents.
fn build_with_layer(name: &str, layer: &str, extra: &str) -> std::path::PathBuf {
    let dir = support::create_temp_workspace(name);
    std::fs::write(dir.join("layer.toml"), layer).expect("layer");
    let build = dir.join("build.toml");
    std::fs::write(
        &build,
        format!("build_name = \"{name}\"\nimports = [\"layer.toml\"]\n{extra}{BASE}"),
    )
    .expect("build");
    build
}

#[test]
fn empty_imported_files_fail_to_load() {
    for contents in ["", "  \n\t\n"] {
        let build = build_with_layer("empty-layer", contents, "");
        let error = try_resolve_config(build.to_str().expect("utf-8 path"))
            .expect_err("an empty layer must fail");
        let message = error.to_string();
        assert!(message.contains("layer.toml"), "{message}");
        assert!(message.contains("is empty"), "{message}");
        let _ = std::fs::remove_dir_all(build.parent().expect("dir"));
    }
}

#[test]
fn layers_that_set_nothing_are_reported() {
    let build = build_with_layer("comment-layer", "# backend payloads\n", "");
    let spec = try_resolve_config(build.to_str().expect("utf-8 path")).expect("config resolves");
    let report = validate_spec(&spec);
    let empty = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "config_layer_empty")
        .collect::<Vec<_>>();
    assert_eq!(empty.len(), 1, "{:?}", report.diagnostics);
    assert!(empty[0].message.contains("layer.toml"));
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let _ = std::fs::remove_dir_all(build.parent().expect("dir"));
}

#[test]
fn expected_items_missing_from_the_build_fail_validation() {
    let build = build_with_layer(
        "expect-layer",
        "[[sources]]\nid = \"backend\"\nkind = \"path\"\npath = \".\"\n",
        "[expect]\nsources = [\"backend\", \"vision-plugin\"]\nartifacts = [\"helios-api\"]\n",
    );
    let spec = try_resolve_config(build.to_str().expect("utf-8 path")).expect("config resolves");
    let report = validate_spec(&spec);
    let missing = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "config_expected_missing")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(missing.len(), 2, "{missing:?}");
    assert!(
        missing
            .iter()
            .any(|message| message.contains("artifact 'helios-api'"))
    );
    assert!(
        missing
            .iter()
            .any(|message| message.contains("source 'vision-plugin'"))
    );
    let _ = std::fs::remove_dir_all(build.parent().expect("dir"));
}
