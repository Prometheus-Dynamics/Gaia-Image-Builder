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

const GROUP_PREAMBLE: &str = r#"
build_name = "build-groups"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[sources]]
id = "engine"
kind = "path"
path = "."

[[sources]]
id = "other"
kind = "path"
path = "."
"#;

fn group_diagnostics(artifacts: &str) -> Vec<(String, String, Option<String>)> {
    let path = write_temp_config(&format!("{GROUP_PREAMBLE}{artifacts}"));
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let _ = fs::remove_file(path);
    validate_spec(&spec)
        .diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.code.starts_with("rust_build_group"))
        .map(|diagnostic| {
            (
                diagnostic.code.to_string(),
                diagnostic.message,
                diagnostic.location,
            )
        })
        .collect()
}

#[test]
fn build_group_members_must_share_cargo_settings() {
    let diagnostics = group_diagnostics(
        r#"
[[artifacts]]
id = "engine"
kind = "rust"
source = "engine"
package = "engine"
build_group = "engine"
output_path = "out/engine"

[[artifacts]]
id = "plugin"
kind = "rust"
source = "other"
package = "plugin"
build_group = "engine"
target = "aarch64-unknown-linux-gnu"
profile = "release"
no_default_features = true
output_path = "out/plugin"
"#,
    );

    for field in ["source", "target", "profile", "no_default_features"] {
        assert!(
            diagnostics.iter().any(
                |(code, message, location)| code == "rust_build_group_conflict"
                    && message.contains(&format!("share {field} "))
                    && message.contains("'engine' and 'plugin'")
                    && location.as_deref() == Some("artifact:plugin")
            ),
            "{field}: {diagnostics:?}"
        );
    }
    assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
}

#[test]
fn consistent_build_groups_and_groups_of_one_are_valid() {
    let diagnostics = group_diagnostics(
        r#"
[[artifacts]]
id = "engine"
kind = "rust"
source = "engine"
package = "engine"
features = ["tls"]
build_group = "engine"
profile = "release"
output_path = "out/engine"

[[artifacts]]
id = "plugin"
kind = "rust"
source = "engine"
package = "plugin"
features = ["simd"]
build_group = "engine"
profile = "release"
output_path = "out/plugin"

[[artifacts]]
id = "solo"
kind = "rust"
source = "other"
package = "solo"
build_group = "solo"
output_path = "out/solo"
"#,
    );

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn build_group_on_a_non_rust_artifact_is_an_error() {
    let path = write_temp_config(&format!(
        "{GROUP_PREAMBLE}{}",
        r#"
[[artifacts]]
id = "web"
kind = "node"
package_dir = "web"
build_group = "engine"
output_path = "out/web"
"#
    ));

    let error = gaia_config::try_resolve_config(path.to_str().expect("temp path utf-8"))
        .expect_err("build_group on a node artifact is rejected");
    let _ = fs::remove_file(path);

    let message = error.to_string();
    assert!(
        message.contains("artifact 'web' sets build_group") && message.contains("kind = \"rust\""),
        "{message}"
    );
}

fn target_codes(target: &str) -> Vec<String> {
    let path = write_temp_config(&format!(
        r#"
build_name = "artifact-targets"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[artifacts]]
id = "lemnosd"
kind = "rust"
package = "lemnosd"
target = "{target}"
output_path = "out/lemnosd"

[[install]]
id = "install-lemnosd"
artifact = "lemnosd"
dest = "/usr/bin/lemnosd"
"#
    ));
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("artifact_target"))
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn installed_artifact_targets_must_be_verifiable_before_building() {
    for target in [
        "aarch64-unknown-linux-musl",
        "aarch64-unknown-linux-gnu",
        "armv7-unknown-linux-musleabihf",
        "x86_64-unknown-linux-musl",
    ] {
        assert!(target_codes(target).is_empty(), "{target}");
    }
    assert_eq!(
        target_codes("mips-unknown-linux-gnu"),
        ["artifact_target_unverifiable"]
    );
}
