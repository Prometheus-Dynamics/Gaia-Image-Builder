//! `[image.signing]` and `[providers.docker] reuse_container`: both are off
//! unless set, and both reach the resolved spec.

pub mod support;

use gaia_config::{ResolveOptions, resolve_config, resolve_config_with_options};
use support::write_temp_config;

const STARTING_POINT: &str = r#"
build_name = "signing-docker-config"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#;

#[test]
fn image_signing_is_off_unless_configured() {
    let plain = write_temp_config(STARTING_POINT);
    let spec = resolve_config(plain.to_str().expect("temp path should be utf-8"));
    assert!(spec.image.signing.is_none());

    let signed = write_temp_config(&format!(
        "{STARTING_POINT}\n[image.signing]\nkind = \"gpg\"\nkey = \"env:GAIA_SIGNING_KEY\"\noutputs = [\"*.img.xz\", \"*.slot.tar\"]\n"
    ));
    let spec = resolve_config(signed.to_str().expect("temp path should be utf-8"));
    let signing = spec.image.signing.expect("signing should be configured");
    assert_eq!(signing.kind, "gpg");
    assert_eq!(signing.key, "env:GAIA_SIGNING_KEY");
    assert_eq!(signing.outputs, ["*.img.xz", "*.slot.tar"]);
}

#[test]
fn docker_reuse_container_is_off_unless_providers_docker_sets_it() {
    let docker_options = ResolveOptions {
        explicit_overrides: vec![("execution.docker.enabled".into(), "true".into())],
        ..ResolveOptions::default()
    };

    let plain = write_temp_config(STARTING_POINT);
    let spec = resolve_config_with_options(
        plain.to_str().expect("temp path should be utf-8"),
        &docker_options,
    );
    let docker = spec.policy.execution.docker.expect("docker backend");
    assert!(!docker.reuse_container);

    let reused = write_temp_config(&format!(
        "{STARTING_POINT}\n[providers.docker]\nreuse_container = true\n"
    ));
    let spec = resolve_config_with_options(
        reused.to_str().expect("temp path should be utf-8"),
        &docker_options,
    );
    let docker = spec.policy.execution.docker.expect("docker backend");
    assert!(docker.reuse_container);
}
