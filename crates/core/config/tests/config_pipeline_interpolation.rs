pub mod support;

use gaia_config::resolve_config;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use support::{write_temp_config, write_temp_config_at};

#[test]
fn interpolates_builtin_root_tokens() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let project_root = std::env::temp_dir().join(format!("gaia-root-tokens-{nonce}"));
    let config_dir = project_root.join("gaia/configs/builds");
    let config_path = config_dir.join("root-token-build.toml");
    std::fs::create_dir_all(&project_root).expect("project root should be created");
    std::fs::write(project_root.join("Cargo.toml"), "[workspace]\n")
        .expect("project marker should be written");
    write_temp_config_at(
        &config_path,
        r#"
build_name = "root-token-build"
version = "2.0.0"

[workspace]
root_dir = "."
build_dir = "gaia/build/${build.name}"
out_dir = "gaia/output/${build.name}"

[[workspace.named_paths]]
alias = "config"
path = "${config.root_dir}"
kind = "host"

[[workspace.named_paths]]
alias = "execution"
path = "${execution.root_dir}"
kind = "host"

[image]
kind = "buildroot"
allow_fallback = true

[[image.expected_images]]
name = "sdcard.img"
format = "raw"
required = true

[image.output]
collect_dir = "gaia/output/${build.name}/images"
archive_name = "${project.root_dir}/output/${build.name}-${build.version}.tar.xz"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
"#,
    );

    let spec = resolve_config(&config_path.display().to_string());

    let expected_archive = project_root
        .join("output/root-token-build-2.0.0.tar.xz")
        .display()
        .to_string();
    assert_eq!(
        spec.image.output.archive_name.as_deref(),
        Some(expected_archive.as_str())
    );
    assert!(
        spec.workspace
            .named_paths
            .iter()
            .any(|path| { path.alias == "config" && Path::new(&path.path) == config_dir })
    );
    assert!(
        spec.workspace
            .named_paths
            .iter()
            .any(|path| { path.alias == "execution" && PathBuf::from(&path.path).is_absolute() })
    );
    let assembly = spec.image.assembly.as_ref().expect("assembly");
    assert_eq!(
        assembly.work_dir.as_deref(),
        Some("gaia/build/root-token-build/assembly")
    );
    assert!(
        spec.policy.interpolation.unresolved.is_empty(),
        "nested workspace interpolation should not leave unresolved tokens"
    );
}

#[test]
fn reporting_post_build_hook_compiles_and_interpolates() {
    let path = write_temp_config(
        r#"
build_name = "hooked"
target = "cm5"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[reporting]
summary = true
provenance = true
manifest = true

[reporting.post_build]
script = "scripts/${build.target}/post-build.sh"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(&path.display().to_string());
    let hook = spec.reporting.post_build.expect("post-build hook");
    assert_eq!(hook.script, "scripts/cm5/post-build.sh");
}
