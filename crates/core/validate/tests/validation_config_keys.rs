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
