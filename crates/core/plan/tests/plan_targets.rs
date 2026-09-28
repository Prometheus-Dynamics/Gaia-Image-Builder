pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{PlanDomain, PlanTarget, plan_build};
use support::{default_config_path, provider_catalogs};

fn operation_ids(plan: &gaia_plan::ExecutionPlan) -> Vec<&str> {
    plan.operations
        .iter()
        .map(|operation| operation.id.as_str())
        .collect()
}

#[test]
fn parses_domains_and_operation_ids() {
    assert_eq!(
        "artifacts".parse::<PlanTarget>(),
        Ok(PlanTarget::Domain(PlanDomain::Artifacts))
    );
    assert_eq!(
        "image".parse::<PlanTarget>(),
        Ok(PlanTarget::Domain(PlanDomain::Image))
    );
    assert_eq!(
        "artifact:gaia-app".parse::<PlanTarget>(),
        Ok(PlanTarget::Operation("artifact:gaia-app".into()))
    );
    let error = "images-please".parse::<PlanTarget>().unwrap_err();
    assert!(error.contains("sources, artifacts, install, stage, image, checkpoints"));
}

#[test]
fn artifacts_target_keeps_artifacts_and_their_dependencies_only() {
    let spec = resolve_config(&default_config_path());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let restricted = plan
        .restrict_to(&[PlanTarget::Domain(PlanDomain::Artifacts)])
        .expect("artifacts exist in the default plan");
    let ids = operation_ids(&restricted);

    assert!(
        restricted.validate().is_empty(),
        "{:?}",
        restricted.validate()
    );
    assert!(ids.contains(&"resolve-build"));
    assert!(ids.iter().any(|id| id.starts_with("artifact:")));
    assert!(
        ids.iter()
            .all(|id| !id.starts_with("image:") && !id.starts_with("install:")),
        "{ids:?}"
    );
    assert!(!ids.contains(&"report:emit"));
    // Every dependency of a kept operation is kept.
    for operation in &restricted.operations {
        for dependency in &operation.depends_on {
            assert!(ids.contains(&dependency.as_str()), "missing {dependency}");
        }
    }
    // Plan order is preserved.
    let full_ids = operation_ids(&plan);
    let positions = ids
        .iter()
        .map(|id| full_ids.iter().position(|full| full == id).unwrap())
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn unknown_operation_and_empty_domain_are_errors() {
    let spec = resolve_config(&default_config_path());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let error = plan
        .restrict_to(&[PlanTarget::Operation("artifact:nope".into())])
        .unwrap_err();
    assert!(error.contains("artifact:nope"));

    if !plan
        .operations
        .iter()
        .any(|operation| PlanDomain::Checkpoints.contains(&operation.kind))
    {
        let error = plan
            .restrict_to(&[PlanTarget::Domain(PlanDomain::Checkpoints)])
            .unwrap_err();
        assert!(error.contains("checkpoints"));
    }
}
