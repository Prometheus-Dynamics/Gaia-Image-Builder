pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

#[test]
fn unknown_checkpoint_anchor_is_rejected() {
    let path = write_temp_config(
        r#"
build_name = "unknown-checkpoint-anchor"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[checkpoints]]
id = "after-missing-install"
backend = "local"
anchor = "install:not-real"
use_policy = "auto"
upload_policy = "off"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_checkpoint_anchor"
                && diagnostic.location.as_deref() == Some("checkpoint:after-missing-install"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn checkpoint_anchor_domain_must_be_in_active_image_feed() {
    let path = write_temp_config(
        r#"
build_name = "checkpoint-anchor-domain"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
install_entries = ["install-gaia-fed"]

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-fed"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[[checkpoints]]
id = "after-install"
backend = "local"
anchor = "install:install-gaia-unfed"
use_policy = "off"
upload_policy = "off"

[[install]]
id = "install-gaia-unfed"
artifact = "gaia-app"
dest = "/usr/bin/gaia-alt"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code
        == "illegal_checkpoint_anchor_domain"
        && diagnostic.location.as_deref() == Some("checkpoint:after-install")));

    let _ = fs::remove_file(path);
}

#[test]
fn required_checkpoint_anchor_outside_image_flow_is_rejected_as_impossible_ordering() {
    let path = write_temp_config(
        r#"
build_name = "checkpoint-anchor-ordering"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
install_entries = ["install-gaia-fed"]

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-fed"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[[checkpoints]]
id = "after-install"
backend = "local"
anchor = "install:install-gaia-unfed"
use_policy = "always"
upload_policy = "off"

[[install]]
id = "install-gaia-unfed"
artifact = "gaia-app"
dest = "/usr/bin/gaia-alt"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code
        == "checkpoint_anchor_impossible_ordering"
        && diagnostic.location.as_deref() == Some("checkpoint:after-install")));

    let _ = fs::remove_file(path);
}
