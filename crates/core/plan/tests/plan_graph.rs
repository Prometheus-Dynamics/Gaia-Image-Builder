pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{
    OperationOptionality, OperationParallelismDomain, OperationParallelismMode, OperationReuse,
    plan_build,
};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyFileSpec, AssemblyFilesystemKindSpec,
    AssemblyFilesystemSpec, AssemblyPartitionTableSpec, AssemblyTransformKindSpec,
    AssemblyTransformSpec, AssemblyTreeSpec, ImageAssemblySpec,
};
use std::fs;
use std::path::PathBuf;
use support::{default_config_path, provider_catalogs, unique_dir};

#[test]
fn default_plan_has_valid_operations_and_rebuild_reasons() {
    let spec = resolve_config(&default_config_path());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();

    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(plan.validate().is_empty());
    assert_eq!(plan.operations.len(), 11);
    assert!(
        plan.operations
            .iter()
            .all(|operation| matches!(operation.reuse, OperationReuse::Execute(_)))
    );
    assert!(plan.operations.iter().any(|operation| {
        matches!(&operation.reuse, OperationReuse::Execute(reason) if reason.code == "artifact_build_required")
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "source:gaia-upstream"
            && operation.parallelism.domain == OperationParallelismDomain::Sources
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && operation.parallelism.domain == OperationParallelismDomain::Artifacts
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "install:install-gaia-app"
            && operation.parallelism.domain == OperationParallelismDomain::Runtime
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "image:build"
            && operation.parallelism.domain == OperationParallelismDomain::Images
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "checkpoint:base-image"
            && operation.parallelism.domain == OperationParallelismDomain::Checkpoints
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "report:emit"
            && operation.parallelism.domain == OperationParallelismDomain::Reporting
            && operation.parallelism.mode == OperationParallelismMode::Exclusive
    }));
}

#[test]
fn starting_point_image_depends_on_declared_source() {
    let root_dir = unique_dir("gaia-plan-starting-point-source-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "starting-point-source"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "base-rootfs"
kind = "git"
repo = "https://example.invalid/base-rootfs.git"
branch = "main"

[providers.git]
allow_remote_resolution = false

[image]
kind = "starting-point"
source = "base-rootfs"
source_path = "rootfs"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let image = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image operation");
    assert!(
        image
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "source:base-rootfs")
    );
}

#[test]
fn buildroot_image_prepare_can_run_before_artifact_installs() {
    let root_dir = unique_dir("gaia-plan-buildroot-split-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "buildroot-split"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "buildroot-source"
kind = "path"
path = "."

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-app"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[image]
kind = "buildroot"
source = "buildroot-source"
defconfig = "qemu_aarch64_virt_defconfig"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let image_prepare = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:prepare")
        .expect("image prepare operation");
    let image_build = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image build operation");

    assert!(
        image_prepare
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "source:buildroot-source")
    );
    assert!(
        !image_prepare
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
    assert!(
        image_build
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "image:prepare")
    );
    assert!(
        image_build
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
}

#[test]
fn image_assembly_depends_on_image_build_when_configured() {
    let root_dir = unique_dir("gaia-plan-image-assembly-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "assembly-plan"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-app"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.feed]
install_entries = ["install-gaia-app"]

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let assembly = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("image assembly operation");
    assert!(
        assembly
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "image:build")
    );
    let image_build = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image build operation");
    assert!(
        image_build
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
    let report = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "report:emit")
        .expect("report operation");
    assert!(
        report
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "image:assembly")
    );
}

#[test]
fn image_assembly_is_not_planned_when_omitted() {
    let spec = resolve_config(&default_config_path());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(
        !plan
            .operations
            .iter()
            .any(|operation| operation.id.as_str() == "image:assembly")
    );
}

#[test]
fn buildroot_and_starting_point_without_assembly_do_not_plan_assembly() {
    for (name, image_toml) in [
        (
            "buildroot-no-assembly-plan",
            r#"
[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#,
        ),
        (
            "starting-point-no-assembly-plan",
            r#"
[image]
kind = "starting-point"
rootfs_path = "/tmp/gaia-missing-rootfs"
rootfs_validation_mode = "allow-missing"
output_mode = "copy-rootfs"
"#,
        ),
    ] {
        let root_dir = unique_dir(name);
        fs::create_dir_all(&root_dir).expect("root dir");
        let config_path = PathBuf::from(&root_dir).join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "{name}"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"
{image_toml}
"#
            ),
        )
        .expect("config");

        let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
        let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
        let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

        assert!(
            !plan
                .operations
                .iter()
                .any(|operation| operation.id.as_str() == "image:assembly"),
            "{name} should not plan image assembly"
        );
    }
}

#[test]
fn image_assembly_fingerprint_tracks_staged_file_inputs() {
    let root_dir = unique_dir("gaia-plan-image-assembly-fingerprint-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let source_path = PathBuf::from(&root_dir).join("config.txt");
    fs::write(&source_path, "one").expect("source");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: Some(source_path.display().to_string().into()),
            src_glob: None,
            dest: "config.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&source_path, "two").expect("updated source");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_glob_expansion() {
    let root_dir = unique_dir("gaia-plan-image-assembly-glob-fingerprint-root");
    let source_dir = PathBuf::from(&root_dir).join("firmware");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("a.dtb"), "one").expect("first dtb");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: None,
            src_glob: Some(source_dir.join("*.dtb").display().to_string().into()),
            dest: ".".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(source_dir.join("b.dtb"), "two").expect("second dtb");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_transform_inputs() {
    let root_dir = unique_dir("gaia-plan-image-assembly-transform-fingerprint-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let source_path = PathBuf::from(&root_dir).join("kernel");
    fs::write(&source_path, "one").expect("source");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_path.display().to_string().into()),
            dest: "$assembly.work/kernel.img".into(),
            deterministic: true,
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&source_path, "two").expect("updated source");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_direct_partition_images() {
    let root_dir = unique_dir("gaia-plan-image-assembly-partition-fingerprint-root");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    let partition_image = gaia_spec::resolve_workspace_path(&spec.workspace, "@assets/rootfs.img")
        .expect("asset path");
    fs::create_dir_all(partition_image.parent().expect("asset parent")).expect("assets dir");
    fs::write(&partition_image, "one").expect("partition image");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$assembly.out/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: "@assets/rootfs.img".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&partition_image, "two").expect("updated partition image");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_provider_root_partition_images() {
    let root_dir = unique_dir("gaia-plan-image-assembly-provider-partition-fingerprint-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    let collect_dir = PathBuf::from(&root_dir).join("out/images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    fs::create_dir_all(&collect_dir).expect("collect dir");
    let partition_image = collect_dir.join("provider-rootfs.img");
    fs::write(&partition_image, "one").expect("provider partition image");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$assembly.out/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: "$provider.images/provider-rootfs.img".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&partition_image, "two").expect("updated provider partition image");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_missing_partition_image_becoming_present() {
    let root_dir = unique_dir("gaia-plan-image-assembly-missing-partition-fingerprint-root");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    let partition_image = gaia_spec::resolve_workspace_path(&spec.workspace, "@assets/rootfs.img")
        .expect("asset path");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$assembly.out/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: "@assets/rootfs.img".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::create_dir_all(partition_image.parent().expect("asset parent")).expect("assets dir");
    fs::write(&partition_image, "now-present").expect("partition image");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_does_not_hash_generated_filesystem_partition_outputs() {
    let root_dir = unique_dir("gaia-plan-image-assembly-generated-partition-fingerprint-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.root_dir = root_dir.clone();
    spec.workspace.build_dir = PathBuf::from(&root_dir).join("build").display().to_string();
    spec.workspace.out_dir = PathBuf::from(&root_dir).join("out").display().to_string();
    let generated_output = PathBuf::from(&root_dir).join("out/images/rootfs.cpio");
    fs::create_dir_all(generated_output.parent().expect("generated parent"))
        .expect("generated dir");
    fs::write(&generated_output, "stale-one").expect("stale generated output");
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: "$assembly.work/rootfs".into(),
        }],
        filesystems: vec![AssemblyFilesystemSpec {
            id: "rootfs".into(),
            kind: AssemblyFilesystemKindSpec::Cpio,
            source_tree: "rootfs".into(),
            output: "$assembly.out/rootfs.cpio".into(),
            size: None,
            deterministic: true,
        }],
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$assembly.out/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: "$assembly.out/rootfs.cpio".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&generated_output, "stale-two").expect("updated stale generated output");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_eq!(first, second);
}

#[test]
fn empty_image_assembly_section_does_not_add_operation() {
    let root_dir = unique_dir("gaia-plan-empty-image-assembly-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "empty-assembly-plan"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(
        !plan
            .operations
            .iter()
            .any(|operation| operation.id.as_str() == "image:assembly")
    );
}

#[test]
fn required_operation_depends_on_best_effort_is_a_plan_error() {
    let plan = gaia_plan::ExecutionPlan {
        build_id: gaia_spec::BuildId::new("bad-plan"),
        operations: vec![
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::checkpoint(&gaia_spec::CheckpointId::new("best-effort")),
                gaia_plan::OperationKind::CaptureCheckpoint {
                    checkpoint_id: gaia_spec::CheckpointId::new("best-effort"),
                },
            )
            .with_optionality(OperationOptionality::BestEffort),
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::report(),
                gaia_plan::OperationKind::EmitReport,
            )
            .with_optionality(OperationOptionality::Required)
            .with_dependency(gaia_plan::OperationId::checkpoint(
                &gaia_spec::CheckpointId::new("best-effort"),
            )),
        ],
    };

    let diagnostics = plan.validate();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "required_depends_on_best_effort" })
    );
}
