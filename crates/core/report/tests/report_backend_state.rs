pub mod support;

use gaia_plan::plan_build;
use gaia_report::generate_report;
use gaia_spec::{AssemblyTreeSpec, ImageAssemblySpec};
use gaia_validate::validate_spec_with_providers;
use std::fs;
use std::path::PathBuf;
use support::{materialize_reusable_outputs, provider_catalogs, test_spec};

#[test]
fn report_generation_tolerates_corrupt_backend_state_lines() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);

    fs::write(
        PathBuf::from(&spec.workspace.build_dir)
            .join("sources/workspace-root/.gaia-source-state.txt"),
        "provider=source.path\nrefresh_policy=never\npin_policy=locked\nbroken-line-without-equals\npath_digest=abc123\n",
    )
    .expect("corrupt source state");
    fs::write(
        PathBuf::from(&spec.artifacts[0].output.path).with_extension("gaia-state.txt"),
        "provider=artifact.rust\nresolved_identifier_kind=package-target\nresolved_identifier=gaia:gaia\nproduced_filename=gaia\noutput_class=binary\nbuild_tool=cargo\nbuild_tool_version=cargo 1.82.0\nbad-line\noutput_sha256=deadbeef\n",
    )
    .expect("corrupt artifact state");
    fs::write(
        PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime/install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nnot-a-pair\ndest=/usr/bin/default\n",
    )
    .expect("corrupt install runtime state");

    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = gaia_exec::ExecutionOutcome::default();

    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert_eq!(
        report.provenance.image_feed_install_entries,
        vec!["install-gaia-app".to_string()]
    );
    assert_eq!(
        report.provenance.image_feed_stage_files,
        vec!["motd".to_string()]
    );
    assert_eq!(
        report.provenance.image_feed_stage_env_sets,
        vec!["runtime-env".to_string()]
    );
    assert_eq!(
        report.provenance.image_feed_stage_services,
        vec!["gaia-service".to_string()]
    );
    assert_eq!(
        report.provenance.image_contract.get("external_tree_mode"),
        Some(&"auto".to_string())
    );
    assert_eq!(
        report.provenance.image_contract.get("config_fragments"),
        Some(&"".to_string())
    );
    assert_eq!(
        report.provenance.image_contract.get("config_overrides"),
        Some(&"".to_string())
    );
    assert_eq!(
        report.provenance.image_contract.get("expected_images"),
        Some(&"rootfs.tar:tar:optional".to_string())
    );
    assert_eq!(
        report.manifest.image_feed_install_entries,
        vec!["install-gaia-app".to_string()]
    );
    assert_eq!(
        report.manifest.image_feed_stage_files,
        vec!["motd".to_string()]
    );
    assert_eq!(
        report.manifest.image_contract.get("external_tree_mode"),
        Some(&"auto".to_string())
    );
    assert_eq!(
        report.manifest.image_contract.get("config_fragments"),
        Some(&"".to_string())
    );
    assert_eq!(
        report.manifest.image_contract.get("config_overrides"),
        Some(&"".to_string())
    );

    let source_state = report
        .provenance
        .source_backend_states
        .iter()
        .find(|record| record.id == "workspace-root")
        .expect("workspace-root state");
    assert_eq!(
        source_state.state.get("provider"),
        Some(&"source.path".to_string())
    );
    assert_eq!(
        source_state.state.get("path_digest"),
        Some(&"abc123".to_string())
    );
    assert_eq!(
        source_state.state.get("refresh_policy"),
        Some(&"never".to_string())
    );
    assert_eq!(
        source_state.state.get("pin_policy"),
        Some(&"locked".to_string())
    );
    assert!(
        !source_state
            .state
            .contains_key("broken-line-without-equals")
    );

    let artifact_state = report
        .provenance
        .artifact_backend_states
        .iter()
        .find(|record| record.id == "gaia-app")
        .expect("gaia-app state");
    assert_eq!(
        artifact_state.state.get("output_sha256"),
        Some(&"deadbeef".to_string())
    );
    assert_eq!(
        artifact_state.state.get("resolved_identifier_kind"),
        Some(&"package-target".to_string())
    );
    assert!(!artifact_state.state.contains_key("bad-line"));

    let artifact_metadata = report
        .provenance
        .artifact_output_metadata
        .iter()
        .find(|record| record.artifact_id == "gaia-app")
        .expect("artifact metadata");
    assert_eq!(
        artifact_metadata.resolved_identifier_kind.as_deref(),
        Some("package-target")
    );
    assert_eq!(
        artifact_metadata.resolved_identifier.as_deref(),
        Some("gaia:gaia")
    );
    assert_eq!(artifact_metadata.output_class.as_deref(), Some("binary"));
    assert_eq!(artifact_metadata.build_tool.as_deref(), Some("cargo"));

    let install_state = report
        .provenance
        .install_backend_states
        .iter()
        .find(|record| record.id == "install-gaia-app")
        .expect("install state");
    assert_eq!(
        install_state.state.get("dest"),
        Some(&"/usr/bin/default".to_string())
    );
    assert!(!install_state.state.contains_key("not-a-pair"));
}

#[test]
fn report_includes_image_assembly_runtime_state() {
    let mut spec = test_spec();
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::write(
        runtime_dir.join("image-assembly.state"),
        "kind=image-assembly\ntree_count=1\nstaged_file_count=2\nskipped_file_count=1\nfile.0.skipped=/tmp/missing-optional\nfile.1.dest=/tmp/boot/config.txt\ncleanup_path_count=2\ncleanup_path.1=/tmp/boot\ncleanup_path.2=/tmp/sdcard.img\ncompleted_busybox_initramfs_count=1\nbusybox.1.dest=/tmp/initramfs/bin/busybox\nbusybox.1.applet_count=2\nbusybox.1.applet.1=sh\nbusybox.1.runtime_linkage=static\nbusybox.1.runtime_library_count=0\ncompleted_transform_count=1\ntransform.1.kind=gzip\ntransform.1.dest=/tmp/boot/kernel.img\ntransform.1.deterministic=true\ntransform.1.bytes=42\ntransform.1.tool=/usr/bin/gzip\ntransform.1.tool_version=gzip 1.13\ncompleted_filesystem_count=1\nfilesystem.1.kind=cpio-gzip\nfilesystem.1.output=/tmp/boot/initramfs\nfilesystem.1.deterministic=true\nfilesystem.1.bytes=128\nfilesystem.1.tool=/usr/bin/gzip\nfilesystem.1.tool_version=gzip 1.13\ncompleted_disk_count=1\ndisk.1.partition_table=mbr\ndisk.1.output=/tmp/sdcard.img\ndisk.1.bytes=2098176\ndisk.1.partition.1.type=0x0C\ndisk.1.partition.1.start_lba=2048\n",
    )
    .expect("assembly runtime state");

    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = gaia_exec::ExecutionOutcome::default();
    let report = generate_report(&spec, &validation, &plan, &outcome);

    let provenance_state = report
        .provenance
        .image_assembly_backend_states
        .iter()
        .find(|record| record.id == "image:assembly")
        .expect("assembly provenance state");
    assert_eq!(
        provenance_state.state.get("staged_file_count"),
        Some(&"2".to_string())
    );
    assert_eq!(
        provenance_state.state.get("skipped_file_count"),
        Some(&"1".to_string())
    );
    assert_eq!(
        provenance_state.state.get("cleanup_path_count"),
        Some(&"2".to_string())
    );
    assert_eq!(
        provenance_state.state.get("transform.1.kind"),
        Some(&"gzip".to_string())
    );
    assert_eq!(
        provenance_state.state.get("transform.1.deterministic"),
        Some(&"true".to_string())
    );
    assert_eq!(
        provenance_state.state.get("transform.1.tool_version"),
        Some(&"gzip 1.13".to_string())
    );
    assert_eq!(
        provenance_state.state.get("filesystem.1.kind"),
        Some(&"cpio-gzip".to_string())
    );
    assert_eq!(
        provenance_state.state.get("filesystem.1.deterministic"),
        Some(&"true".to_string())
    );
    assert_eq!(
        provenance_state.state.get("busybox.1.applet_count"),
        Some(&"2".to_string())
    );
    assert_eq!(
        provenance_state.state.get("busybox.1.runtime_linkage"),
        Some(&"static".to_string())
    );
    assert_eq!(
        provenance_state.state.get("disk.1.partition_table"),
        Some(&"mbr".to_string())
    );
    assert_eq!(
        provenance_state.state.get("disk.1.partition.1.start_lba"),
        Some(&"2048".to_string())
    );

    let manifest_state = report
        .manifest
        .image_assembly
        .iter()
        .find(|record| record.id == "image:assembly")
        .expect("assembly manifest state");
    assert_eq!(
        manifest_state.state.get("file.1.dest"),
        Some(&"/tmp/boot/config.txt".to_string())
    );
    assert_eq!(
        manifest_state.state.get("file.0.skipped"),
        Some(&"/tmp/missing-optional".to_string())
    );
    assert_eq!(
        manifest_state.state.get("cleanup_path.1"),
        Some(&"/tmp/boot".to_string())
    );
    assert_eq!(
        manifest_state.state.get("transform.1.bytes"),
        Some(&"42".to_string())
    );
    assert_eq!(
        manifest_state.state.get("filesystem.1.bytes"),
        Some(&"128".to_string())
    );
    assert_eq!(
        manifest_state.state.get("busybox.1.dest"),
        Some(&"/tmp/initramfs/bin/busybox".to_string())
    );
    assert_eq!(
        manifest_state.state.get("busybox.1.runtime_library_count"),
        Some(&"0".to_string())
    );
    assert_eq!(
        manifest_state.state.get("disk.1.bytes"),
        Some(&"2098176".to_string())
    );
    assert_eq!(
        manifest_state.state.get("disk.1.partition.1.type"),
        Some(&"0x0C".to_string())
    );
}

#[test]
fn report_includes_output_hygiene_warnings_without_failing() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);
    let collect_dir = PathBuf::from(spec.image.output.collect_dir.as_ref().expect("collect dir"));
    fs::create_dir_all(collect_dir.join("build")).expect("transient build dir");
    let large_file = collect_dir.join("download-cache.tar");
    let file = fs::File::create(&large_file).expect("large file");
    file.set_len(101 * 1024 * 1024).expect("sparse large file");

    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = gaia_exec::ExecutionOutcome::default();
    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert_eq!(report.execution_failures.len(), 0);
    assert_eq!(report.summary.warning_count, validation.warnings.len() + 2);
    assert!(
        report
            .provenance
            .output_hygiene_warnings
            .iter()
            .any(|warning| warning.code == "publish_transient_directory"
                && warning.path.ends_with("build"))
    );
    assert!(
        report
            .manifest
            .output_hygiene_warnings
            .iter()
            .any(|warning| warning.code == "publish_large_unexpected_file"
                && warning.path.ends_with("download-cache.tar")
                && warning.size_bytes == Some(101 * 1024 * 1024))
    );
}

#[test]
fn output_hygiene_uses_custom_threshold_and_transient_names() {
    let mut spec = test_spec();
    spec.reporting.output_hygiene.large_file_threshold_bytes = 8;
    spec.reporting.output_hygiene.transient_dir_names = vec!["tmp-work".into()];
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);
    let collect_dir = PathBuf::from(spec.image.output.collect_dir.as_ref().expect("collect dir"));
    fs::create_dir_all(collect_dir.join("build")).expect("default transient dir");
    fs::create_dir_all(collect_dir.join("tmp-work")).expect("custom transient dir");
    fs::write(collect_dir.join("small-cache.bin"), b"12345678").expect("small cache file");

    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = gaia_exec::ExecutionOutcome::default();
    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert!(
        report
            .manifest
            .output_hygiene_warnings
            .iter()
            .any(|warning| warning.code == "publish_transient_directory"
                && warning.path.ends_with("tmp-work"))
    );
    assert!(
        report
            .manifest
            .output_hygiene_warnings
            .iter()
            .all(|warning| !warning.path.ends_with("build")),
        "default transient names should be replaced by custom names"
    );
    assert!(
        report
            .manifest
            .output_hygiene_warnings
            .iter()
            .any(|warning| warning.code == "publish_large_unexpected_file"
                && warning.path.ends_with("small-cache.bin")
                && warning.size_bytes == Some(8))
    );
}
