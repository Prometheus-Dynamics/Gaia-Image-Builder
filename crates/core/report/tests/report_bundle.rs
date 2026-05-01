pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::plan_build;
use gaia_report::{ReportFileKind, generate_report, write_report_bundle};
use gaia_validate::validate_spec_with_providers;
use support::{provider_catalogs, test_spec};

#[test]
fn generates_report_bundle_for_default_run() {
    let mut spec = test_spec();
    if let gaia_spec::ImageDefinition::Buildroot(buildroot) = &mut spec.image.definition {
        buildroot.allow_fallback = true;
    }
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert_eq!(report.summary.operation_count, 11);
    assert_eq!(report.summary.completed_operations, 11);
    assert_eq!(report.summary.rolled_back_operations, 0);
    assert!(report.summary.rollback_on_error);
    assert!(!report.summary.preserve_failed_outputs);
    assert!(
        report
            .summary
            .primary_image_output
            .as_deref()
            .is_some_and(|path| path.contains("/images/"))
    );
    assert_eq!(
        report.summary.rollback_domains,
        vec![
            "sources",
            "artifacts",
            "installs",
            "stage",
            "images",
            "checkpoints",
        ]
    );
    assert_eq!(report.summary.checkpoint_count, 1);
    assert_eq!(report.summary.checkpoint_built_count, 1);
    assert_eq!(report.summary.checkpoint_reused_count, 0);
    assert!(report.summary.failure_classes.is_empty());
    assert!(report.execution_failures.is_empty());
    assert_eq!(
        report.summary.build_description.as_deref(),
        Some("Gaia rewrite default build for workspace-level image assembly.")
    );
    assert!(
        report
            .selection
            .selected_build_file
            .as_deref()
            .is_some_and(|path| path.ends_with("examples/default-workspace/configs/default.toml"))
    );
    assert_eq!(
        report.selection.selected_preset.as_deref(),
        Some("rewrite-dev")
    );
    assert_eq!(
        report.selection.selected_env_files,
        vec!["runtime.env".to_string()]
    );
    assert!(report.selection.rollback_on_error);
    assert!(!report.selection.preserve_failed_outputs);
    assert_eq!(
        report.selection.rollback_domains,
        report.summary.rollback_domains
    );
    assert_eq!(
        report.selection.precedence_order,
        vec![
            "ConfigDefaults".to_string(),
            "SelectedPreset".to_string(),
            "EnvFiles".to_string(),
            "InlineEnv".to_string(),
            "ProcessEnv".to_string(),
            "CliSetOverrides".to_string(),
        ]
    );
    assert!(
        report
            .provenance
            .selected_build_file
            .as_deref()
            .is_some_and(|path| path.ends_with("examples/default-workspace/configs/default.toml"))
    );
    assert_eq!(
        report.provenance.selected_preset.as_deref(),
        Some("rewrite-dev")
    );
    assert_eq!(
        report.provenance.selected_env_files,
        vec!["runtime.env".to_string()]
    );
    assert!(report.provenance.rollback_on_error);
    assert!(!report.provenance.preserve_failed_outputs);
    assert_eq!(
        report.provenance.rollback_domains,
        report.summary.rollback_domains
    );
    assert_eq!(
        report.provenance.precedence_order,
        vec![
            "ConfigDefaults".to_string(),
            "SelectedPreset".to_string(),
            "EnvFiles".to_string(),
            "InlineEnv".to_string(),
            "ProcessEnv".to_string(),
            "CliSetOverrides".to_string(),
        ]
    );
    assert!(
        report
            .provenance
            .precedence_layers
            .iter()
            .any(|layer| layer.source == "EnvFiles")
    );
    assert_eq!(report.provenance.product_family.as_deref(), Some("gaia"));
    assert_eq!(
        report.provenance.product_name.as_deref(),
        Some("image-builder")
    );
    assert_eq!(
        report.provenance.product_sku.as_deref(),
        Some("gaia-rewrite-dev")
    );
    assert!(
        report
            .provenance
            .metadata_labels
            .iter()
            .any(|(key, value)| key == "stack" && value == "rewrite")
    );
    assert_eq!(
        report.provenance.identity_project.as_deref(),
        Some("gaia-image-builder")
    );
    assert_eq!(
        report.provenance.identity_vendor.as_deref(),
        Some("Prometheus Dynamics")
    );
    assert_eq!(
        report.provenance.identity_channel.as_deref(),
        Some("rewrite")
    );
    assert!(
        report
            .provenance
            .identity_labels
            .iter()
            .any(|(key, value)| key == "branch" && value == "main")
    );
    assert_eq!(report.provenance.source_providers.len(), 2);
    assert_eq!(report.provenance.artifact_providers.len(), 1);
    assert!(
        report
            .provenance
            .artifact_install_identities
            .iter()
            .any(|record| record.artifact_id == "gaia-app"
                && record.install_name == "default"
                && record.install_class == "binary"
                && record.destination_hint.as_deref() == Some("/usr/bin/default"))
    );
    assert_eq!(report.provenance.image_output_collect_dirs.len(), 1);
    assert_eq!(report.provenance.image_output_archives.len(), 1);
    assert!(
        report
            .provenance
            .image_output_archives
            .iter()
            .any(|path| path.contains("/images/"))
    );
    assert!(report
        .provenance
        .source_backend_states
        .iter()
        .any(|record| record.id == "workspace-root" && record.state.contains_key("path_digest")));
    assert!(
        report
            .provenance
            .artifact_backend_states
            .iter()
            .any(|record| record.id == "gaia-app" && record.state.contains_key("output_sha256"))
    );
    assert!(
        report
            .provenance
            .artifact_output_metadata
            .iter()
            .any(|record| record.artifact_id == "gaia-app"
                && record.resolved_identifier_kind.as_deref() == Some("package-target")
                && record.resolved_identifier.as_deref() == Some("gaia:gaia")
                && record.output_class.as_deref() == Some("binary")
                && record.build_tool.as_deref() == Some("cargo"))
    );
    assert!(
        report
            .provenance
            .image_backend_states
            .iter()
            .any(|record| record.id == "image.buildroot"
                && record.state.contains_key("collect_digest"))
    );
    assert!(
        report
            .provenance
            .install_backend_states
            .iter()
            .any(|record| record.id == "install-gaia-app"
                && record.state.get("dest") == Some(&"/usr/bin/default".to_string()))
    );
    assert!(
        report
            .provenance
            .stage_file_backend_states
            .iter()
            .any(|record| record.id == "motd"
                && record.state.get("dest") == Some(&"/etc/motd".to_string()))
    );
    assert!(
        report
            .provenance
            .checkpoint_backend_states
            .iter()
            .any(|record| record.id == "base-image"
                && record.state.get("backend") == Some(&"local".to_string()))
    );
    assert_eq!(report.manifest.operations.len(), 11);
    assert!(report.manifest.rollback_on_error);
    assert!(!report.manifest.preserve_failed_outputs);
    assert_eq!(
        report.manifest.rollback_domains,
        report.summary.rollback_domains
    );
    assert_eq!(report.manifest.sources.len(), 2);
    assert_eq!(report.manifest.artifacts.len(), 1);
    assert_eq!(report.manifest.image_outputs.len(), 1);
    assert!(report.manifest.sources.iter().any(|record| {
        record.id == "gaia-upstream"
            && record.backend_state.contains_key("provider")
            && record.backend_state.get("build_branch") == Some(&"main".to_string())
    }));
    assert!(report.manifest.artifacts.iter().any(|record| {
        record.id == "gaia-app"
            && record.resolved_identifier_kind.as_deref() == Some("package-target")
            && record.resolved_identifier.as_deref() == Some("gaia:gaia")
            && record.produced_filename.is_some()
            && record.output_class.as_deref() == Some("binary")
            && record.build_tool.as_deref() == Some("cargo")
            && record.build_tool_version.is_some()
            && record.backend_state.contains_key("output_sha256")
            && record.backend_state.get("build_profile") == Some(&"dev".to_string())
            && record.install_name.as_deref() == Some("default")
            && record.install_class.as_deref() == Some("binary")
            && record.install_destination_hint.as_deref() == Some("/usr/bin/default")
    }));
    assert!(report.manifest.installs.iter().any(|record| {
        record.id == "install-gaia-app"
            && record.backend_state.get("dest") == Some(&"/usr/bin/default".to_string())
    }));
    assert!(report.manifest.stage_files.iter().any(|record| {
        record.id == "motd"
            && record.backend_state.get("dest") == Some(&"/etc/motd".to_string())
            && record.origin == "static-asset"
    }));
    assert!(report.manifest.checkpoints.iter().any(|record| {
        record.id == "base-image"
            && record.backend_state.get("backend") == Some(&"local".to_string())
            && record.anchor == "image"
    }));
    assert!(report.manifest.image_outputs.iter().any(|record| {
        record.provider_id == "image.buildroot"
            && record.backend_state.contains_key("archive_sha256")
            && record.backend_state.get("build_target") == Some(&"cm5".to_string())
    }));
    assert_eq!(report.rebuild_reasons.len(), 12);
}

#[test]
fn writes_report_bundle_files_with_sizes() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    let report = generate_report(&spec, &validation, &plan, &outcome);

    let written = write_report_bundle(&spec, &report).expect("report files should be written");

    assert_eq!(written.files.len(), 5);
    assert!(written.files.iter().all(|file| file.bytes > 0));
    assert!(
        written
            .files
            .iter()
            .any(|file| matches!(file.kind, ReportFileKind::Summary))
    );
    assert!(
        written
            .files
            .iter()
            .any(|file| matches!(file.kind, ReportFileKind::Selection))
    );
}
