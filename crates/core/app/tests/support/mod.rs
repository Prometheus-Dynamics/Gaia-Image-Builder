use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{operation_output_signature, plan_build, spec_fingerprint};
use gaia_source_providers::SourceProviderCatalog;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn config_path() -> String {
    format!(
        "{}/../../../examples/default-workspace/configs/default.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn unique_dir(prefix: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}"))
        .display()
        .to_string()
}

pub fn write_temp_build(contents: &str) -> String {
    let root = std::env::temp_dir().join("gaia-tests");
    fs::create_dir_all(&root).expect("test scratch root");
    let path = root.join(format!(
        "gaia-cli-build-{}.toml",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    fs::write(&path, contents).expect("temp build config");
    path.display().to_string()
}

pub fn seed_default_assets(root_dir: &str) {
    let assets_dir = PathBuf::from(root_dir).join("examples/default-workspace/assets");
    fs::create_dir_all(assets_dir.join("etc")).expect("assets/etc");
    fs::create_dir_all(assets_dir.join("systemd")).expect("assets/systemd");
    fs::write(assets_dir.join("etc/motd"), "Gaia test image\n").expect("motd");
    fs::write(
        assets_dir.join("systemd/gaia.service"),
        "[Unit]\nDescription=Gaia Test\n[Service]\nExecStart=/usr/bin/default\n",
    )
    .expect("service unit");
}

pub fn provider_catalogs() -> (
    SourceProviderCatalog,
    ArtifactProviderCatalog,
    ImageProviderCatalog,
) {
    gaia_default_providers::provider_catalogs()
}

pub fn materialize_reusable_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream"))
        .expect("gaia-upstream source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
        "ok",
    )
    .expect("gaia-upstream source marker");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir)
            .join("sources/gaia-upstream/.gaia-source-state.txt"),
        "provider=source.git\nsource=gaia-upstream\n",
    )
    .expect("gaia-upstream source state");
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root"))
        .expect("workspace-root source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
        "ok",
    )
    .expect("workspace-root source marker");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir)
            .join("sources/workspace-root/.gaia-source-state.txt"),
        "provider=source.path\nsource=workspace-root\n",
    )
    .expect("workspace-root source state");
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    fs::write(
        PathBuf::from(&spec.artifacts[0].output.path).with_extension("gaia-state.txt"),
        "provider=artifact.rust\nartifact=gaia-app\noutput=test\n",
    )
    .expect("artifact state");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir create");
    fs::write(
        PathBuf::from(&collect_dir).join("image-provider.txt"),
        "image",
    )
    .expect("image marker");
    fs::write(
        PathBuf::from(&collect_dir).join(".gaia-image-state.txt"),
        "provider=image.buildroot\nemit_report=true\n",
    )
    .expect("image state");
    let archive_name = spec
        .image
        .output
        .archive_name
        .clone()
        .expect("image archive name");
    fs::write(PathBuf::from(&collect_dir).join(archive_name), "archive").expect("image archive");
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\nreplace=true\nmode=755\nowner=root\ngroup=root\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\nsrc=assets/motd\ndest=/etc/motd\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia\nunit_path=/etc/systemd/system/gaia.service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\nuse_policy=Auto\nupload_policy=Off\n",
    )
    .expect("checkpoint runtime state");
}

pub fn seed_reuse_state(root_dir: &str, build_dir: &str, out_dir: &str) {
    let spec = resolve_config_with_options(
        &config_path(),
        &ResolveOptions {
            preset: Some("ci".into()),
            env_files: vec!["examples/default-workspace/configs/runtime.env".into()],
            env_overrides: vec![
                ("API_TOKEN".into(), "super-secret-token".into()),
                ("GAIA_MODE".into(), "ci-env".into()),
            ],
            explicit_overrides: vec![
                ("env.DB_PASSWORD".into(), "ultra-secret-password".into()),
                ("build.version".into(), "9.9.9".into()),
                ("workspace.root_dir".into(), root_dir.into()),
                ("workspace.out_dir".into(), out_dir.into()),
                ("workspace.build_dir".into(), build_dir.into()),
            ],
        },
    );
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = [
        "source:gaia-upstream",
        "source:workspace-root",
        "artifact:gaia-app",
        "install:install-gaia-app",
        "stage:file:motd",
        "stage:env:runtime-env",
        "stage:service:gaia-service",
        "image:build",
        "checkpoint:base-image",
    ];
    let mut body = format!("fingerprint={}\n", spec_fingerprint(&spec));
    let completed = reused_ids
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for operation_id in &completed {
        body.push_str(operation_id);
        body.push('\n');
    }
    for operation in &baseline_plan.operations {
        if completed.contains(operation.id.as_str()) {
            body.push_str(&format!(
                "op={};{}\n",
                operation.id.as_str(),
                operation.fingerprint
            ));
            if let Some(signature) = operation_output_signature(&spec, &operation.kind) {
                body.push_str(&format!("out={};{}\n", operation.id.as_str(), signature));
            }
        }
    }
    let state_path = PathBuf::from(out_dir)
        .join(".gaia")
        .join(format!("{}.reuse-state", spec.build_name()));
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent).expect("reuse state dir");
    }
    fs::write(state_path, body).expect("reuse state write");
}

pub fn polyglot_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-polyglot-squashfs/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn smoke_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-rust-minimal/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn starting_point_example_build_path() -> String {
    format!(
        "{}/../../../examples/imported-rootfs-minimal/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn starting_point_raw_image_example_build_path() -> String {
    format!(
        "{}/../../../examples/imported-raw-image-mutate/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn starting_point_git_project_example_build_path() -> String {
    format!(
        "{}/../../../examples/imported-rootfs-rust-git/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn starting_point_polyglot_git_example_build_path() -> String {
    format!(
        "{}/../../../examples/imported-rootfs-polyglot-git/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn starting_point_cross_target_git_example_build_path() -> String {
    format!(
        "{}/../../../examples/imported-rootfs-cross-aarch64/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn squashfs_smoke_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-rust-squashfs/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn sdcard_smoke_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-rust-sdcard/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn raspberrypi4_go_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-raspberrypi4-go/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub fn rust_aarch64_example_build_path() -> String {
    format!(
        "{}/../../../examples/buildroot-rust-aarch64/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}
