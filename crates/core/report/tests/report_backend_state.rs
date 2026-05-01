pub mod support;

use gaia_plan::plan_build;
use gaia_report::generate_report;
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
