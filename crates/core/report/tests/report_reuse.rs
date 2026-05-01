pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{plan_build, plan_build_with_reuse_state};
use gaia_report::generate_report;
use gaia_validate::validate_spec_with_providers;
use support::{materialize_reusable_outputs, provider_catalogs, reuse_state_for_ids, test_spec};

#[test]
fn generates_report_bundle_with_reuse_counts() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
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
    let reuse_state = reuse_state_for_ids(&spec, &baseline_plan, &reused_ids);
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&reuse_state),
    );
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

    assert_eq!(report.summary.reused_operations, 9);
    assert_eq!(report.summary.checkpoint_built_count, 0);
    assert_eq!(report.summary.checkpoint_reused_count, 1);
    assert!(report.rebuild_reasons.len() >= 2);
}
