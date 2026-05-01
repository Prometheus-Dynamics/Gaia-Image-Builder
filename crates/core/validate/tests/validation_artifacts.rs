pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

#[test]
fn empty_custom_build_mode_is_an_error() {
    let path = write_temp_config(
        r#"
build_name = "invalid-build-mode"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "bad-artifact"
kind = "rust"
package = "gaia"
profile = ""
output_path = "out/bad-artifact"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "artifact_build_mode_empty"
                && diagnostic.location.as_deref() == Some("artifact:bad-artifact"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn empty_artifact_target_is_an_error() {
    let path = write_temp_config(
        r#"
build_name = "invalid-artifact-target"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "bad-target"
kind = "rust"
package = "gaia"
target = ""
output_path = "out/bad-target"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "artifact_target_empty"
                && diagnostic.location.as_deref() == Some("artifact:bad-target"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn duplicate_artifact_install_identities_are_rejected() {
    let path = write_temp_config(
        r#"
build_name = "duplicate-install-identity"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "gaia-a"
kind = "rust"
package = "gaia"
install_name = "gaia"
install_class = "binary"
install_dest_hint = "/usr/bin/gaia"
output_path = "out/gaia-a"

[[artifacts]]
id = "gaia-b"
kind = "rust"
package = "gaia"
install_name = "gaia"
install_class = "binary"
install_dest_hint = "/usr/bin/gaia"
output_path = "out/gaia-b"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code
        == "duplicate_artifact_install_identity"
        && diagnostic.location.as_deref() == Some("artifact:gaia-b")));

    let _ = fs::remove_file(path);
}
