pub mod support;

use gaia_config::resolve_config;
use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::plan_build;
use gaia_report::{generate_report, mask_value};
use gaia_validate::validate_spec_with_providers;
use support::{default_config_path, provider_catalogs, unique_dir};

#[test]
fn masks_sensitive_env_values_in_provenance() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            preset: Some("ci".into()),
            env_files: vec!["runtime.env".into()],
            env_overrides: vec![
                ("API_TOKEN".into(), "super-secret-token".into()),
                ("GAIA_MODE".into(), "ci-env".into()),
            ],
            explicit_overrides: vec![
                ("env.DB_PASSWORD".into(), "ultra-secret-password".into()),
                ("build.version".into(), "9.9.9".into()),
            ],
        },
    );
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

    assert!(
        report
            .selection
            .selected_env_overrides
            .iter()
            .any(|(key, value)| key == "API_TOKEN" && value == "***")
    );
    assert!(
        report
            .selection
            .selected_env_overrides
            .iter()
            .any(|(key, value)| key == "GAIA_MODE" && value == "ci-env")
    );
    assert!(
        report
            .selection
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "env.DB_PASSWORD" && value == "***")
    );
    assert!(
        report
            .selection
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "build.version" && value == "9.9.9")
    );
    assert!(
        report
            .provenance
            .selected_env_overrides
            .iter()
            .any(|(key, value)| key == "API_TOKEN" && value == "***")
    );
    assert!(
        report
            .provenance
            .selected_env_overrides
            .iter()
            .any(|(key, value)| key == "GAIA_MODE" && value == "ci-env")
    );
    assert!(
        report
            .provenance
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "env.DB_PASSWORD" && value == "***")
    );
    assert!(
        report
            .provenance
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "build.version" && value == "9.9.9")
    );
}

#[test]
fn leaves_values_visible_when_masking_is_disabled() {
    let mut spec = resolve_config(&default_config_path());
    spec.workspace.build_dir = unique_dir("gaia-report-build");
    spec.workspace.out_dir = unique_dir("gaia-report-out");
    spec.reporting.masking.enabled = false;

    assert_eq!(
        mask_value("API_TOKEN", "super-secret-token", &spec.reporting),
        "super-secret-token"
    );
}
