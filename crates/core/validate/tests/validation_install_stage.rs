pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::{create_temp_workspace, write_temp_config};

#[test]
fn install_and_stage_dest_collisions_are_rejected() {
    let path = write_temp_config(
        r#"
build_name = "dest-collision"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[[stage.files]]
id = "gaia-file"
src = "assets/gaia"
dest = "/usr/bin/gaia"
origin = "static-asset"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "image_destination_conflict")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn duplicate_stage_runtime_destinations_are_rejected() {
    let path = write_temp_config(
        r#"
build_name = "stage-runtime-collision"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.env_sets]]
id = "runtime-a"
name = "runtime"
entries = [["MODE", "a"]]

[[stage.env_sets]]
id = "runtime-b"
name = "runtime"
entries = [["MODE", "b"]]

[[stage.services]]
id = "service-a"
name = "gaia.service"
unit_path = "assets/gaia-a.service"

[[stage.services]]
id = "service-b"
name = "gaia.service"
unit_path = "assets/gaia-b.service"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    let conflicts = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "image_destination_conflict")
        .count();
    assert!(
        conflicts >= 2,
        "expected env-set and service destination conflicts"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn stage_file_collisions_with_runtime_derived_paths_are_rejected() {
    let path = write_temp_config(
        r#"
build_name = "stage-runtime-derived-collision"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.files]]
id = "runtime-env-file"
src = "assets/runtime.env"
dest = "/etc/default/runtime.env"
origin = "static-asset"

[[stage.files]]
id = "service-file"
src = "assets/gaia.service"
dest = "/etc/systemd/system/gaia.service"
origin = "static-asset"

[[stage.env_sets]]
id = "runtime"
name = "runtime"
entries = [["MODE", "prod"]]

[[stage.services]]
id = "gaia-service"
name = "gaia.service"
unit_path = "assets/gaia.service"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    let conflicts = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "image_destination_conflict")
        .count();
    assert!(
        conflicts >= 2,
        "expected stage file collisions with derived env/service destinations"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn missing_static_stage_file_source_is_an_error() {
    let root = create_temp_workspace("gaia-stage-file-missing");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "missing-stage-file-src"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.files]]
id = "missing-file"
src = "assets/does-not-exist"
dest = "/etc/missing"
origin = "static-asset"
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 path"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "stage_file_src_missing")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn static_stage_directory_source_is_allowed() {
    let root = create_temp_workspace("gaia-stage-dir-present");
    fs::create_dir_all(root.join("assets/frontend-build")).expect("stage dir");
    fs::write(root.join("assets/frontend-build/index.html"), "ok").expect("stage file");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "present-stage-dir-src"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.files]]
id = "frontend-bundle"
src = "assets/frontend-build"
dest = "/opt/frontend"
origin = "static-asset"
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 path"));
    let report = validate_spec(&spec);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "stage_file_src_missing"),
        "directory-backed static stage source should validate"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_stage_service_unit_path_is_an_error() {
    let root = create_temp_workspace("gaia-stage-service-missing");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "missing-stage-service-unit"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.services]]
id = "demo-service"
name = "demo.service"
unit_path = "assets/demo.service"
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 path"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "stage_service_unit_missing")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn busybox_hostile_stage_script_patterns_emit_a_warning() {
    let root = create_temp_workspace("gaia-stage-script-busybox");
    fs::create_dir_all(root.join("assets/etc/init.d")).expect("assets dir");
    fs::write(
        root.join("assets/etc/init.d/S50demo"),
        r#"#!/bin/sh
wget --method DELETE "http://127.0.0.1:5800/api/foo"
awk 'match($0, /foo{2,}/) { print $0 }' /tmp/input
"#,
    )
    .expect("script");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "busybox-script-warning"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.files]]
id = "demo-script"
src = "assets/etc/init.d/S50demo"
dest = "/etc/init.d/S50demo"
origin = "static-asset"
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 path"));
    let report = validate_spec(&spec);

    let busybox_warnings = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "stage_script_busybox_portability_risk")
        .count();
    assert!(
        busybox_warnings >= 2,
        "expected busybox portability warnings for wget and awk patterns"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn systemd_socket_mount_and_target_unit_names_are_valid_stage_services() {
    let root = create_temp_workspace("gaia-systemd-unit-suffixes");
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    for unit in ["demo.socket", "data.mount", "provision.target"] {
        fs::write(root.join("assets").join(unit), "[Unit]\nDescription=demo\n").expect("unit");
    }
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "systemd-unit-suffixes"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.services]]
id = "demo-socket"
name = "demo.socket"
unit_path = "assets/demo.socket"

[[stage.services]]
id = "data-mount"
name = "data.mount"
unit_path = "assets/data.mount"

[[stage.services]]
id = "provision-target"
name = "provision.target"
unit_path = "assets/provision.target"
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 path"));
    let report = validate_spec(&spec);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "stage_service_name_unusual" })
    );

    let _ = fs::remove_dir_all(root);
}
