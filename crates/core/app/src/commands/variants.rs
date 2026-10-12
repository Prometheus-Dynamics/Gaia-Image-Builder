//! Builds that declare `[[variants]]`: one image per variant, from one
//! shared base.
//!
//! Every declared variant is resolved and planned, so which operations are
//! shared never depends on the `--variant` selection. An operation is
//! **shared** when it is the same in every variant (same fingerprint and
//! dependencies) and everything it depends on is shared too. Shared
//! operations run once, in the first variant that needs them, and are
//! recorded in the shared state file `<build>.reuse-state`. Every other
//! operation is **variant-specific**:
//! - an artifact writes its output under `variants/<id>/` next to its
//!   configured path, so variants never overwrite each other;
//! - install and stage records, checkpoints, the Buildroot image build and
//!   disk assembly always run per variant (they feed the image, which
//!   differs), and their records live in `.gaia/runtime@<id>`;
//! - the image's default collect dir is `<out>/images/buildroot/<id>`;
//! - its reuse state is `<build>@<id>.reuse-state`, which also keeps the
//!   shared entries the variant reused, so it is complete on its own.
//!
//! A source a variant checks out differently (another rev, say) is
//! **variant-specific** too: it lives under `sources/variants/<id>/<source>`,
//! and the config of every variant is resolved again with that scope so
//! `${source.<id>.path}` and `${source.<id>.commit}` name the variant's own
//! checkout. A source the Buildroot base (`image:prepare`) or the image
//! reads directly must match in every variant, since the base tree is shared.
//!
//! Variants run one after another in one process. A failing variant stops
//! the run; the variants before it keep their outputs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use gaia_config::{ResolveOptions, resolve_variant_specs, variant_output_collisions};
use gaia_exec::ExecutionOutcome;
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, PlanTarget, RebuildRequest, ReuseState,
    plan_build_with_rebuilds,
};
use gaia_spec::{DEFAULT_VARIANT_ID, ResolvedBuildSpec};
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::run::{collect_run_artifacts_for_spec, run_outcome};
use super::{CommandOutcome, VariantOutcome, load_reuse_state, save_reuse_state};

/// Which reuse state an operation of a build reads and writes.
#[derive(Clone, Copy)]
pub(crate) enum ReuseScope<'a> {
    /// A build without variants: its own state file.
    Build,
    /// One variant of a build with variants. Operations in `shared` come
    /// from the shared state; the others from the variant's own state.
    Variant { shared: &'a BTreeSet<String> },
}

/// The build file a variant build is resolved from, and the options.
#[derive(Clone, Copy)]
pub(crate) struct VariantSource<'a> {
    pub(crate) build: &'a str,
    pub(crate) options: &'a ResolveOptions,
}

/// The planned variants, with which operations each one shares.
#[derive(Debug)]
pub(crate) struct VariantSplit {
    /// Operation ids that are the same in every variant.
    pub(crate) shared: BTreeSet<String>,
    /// Source ids checked out per variant (their materialization differs).
    pub(crate) variant_sources: BTreeSet<String>,
    /// Per variant in spec order: its operation count.
    pub(crate) operation_counts: Vec<(String, usize)>,
}

/// The variant id of a spec (`default` for the base config).
pub(crate) fn variant_id(spec: &ResolvedBuildSpec) -> &str {
    spec.selection
        .variant
        .as_deref()
        .unwrap_or(DEFAULT_VARIANT_ID)
}

/// Validates, classifies and scopes every declared variant. Errors are
/// reported before anything runs. `build` and `options` resolve the specs
/// again once it is known which sources each variant checks out itself.
pub(crate) fn prepare_variants(
    context: &AppContext,
    source: VariantSource<'_>,
    mut specs: Vec<ResolvedBuildSpec>,
) -> Result<(Vec<ResolvedBuildSpec>, VariantSplit), String> {
    for spec in specs.iter() {
        let validation = validate_spec_with_providers(
            spec,
            &context.source_catalog,
            &context.artifact_catalog,
            &context.image_catalog,
        );
        if !validation.errors.is_empty() {
            return Err(format!(
                "variant '{}' of build '{}' has {} validation error(s); run 'gaia validate' with it to see them",
                variant_id(spec),
                specs[0].display_name(),
                validation.errors.len()
            ));
        }
    }
    let split = classify(context, &specs)?;
    if !split.variant_sources.is_empty() {
        specs = gaia_config::resolve_variant_specs_scoped(
            source.build,
            source.options,
            &split.variant_sources,
        )
        .map_err(|error| error.to_string())?;
    }
    for spec in specs.iter_mut() {
        scope_variant(spec, &split.shared);
    }
    let collisions = variant_output_collisions(&specs);
    if !collisions.is_empty() {
        return Err(collisions.join("\n"));
    }
    Ok((specs, split))
}

/// Plans every variant without reuse state and splits its operations.
fn classify(context: &AppContext, specs: &[ResolvedBuildSpec]) -> Result<VariantSplit, String> {
    let plans = specs
        .iter()
        .map(|spec| {
            plan_build_with_rebuilds(
                spec,
                &context.source_catalog,
                &context.artifact_catalog,
                &context.image_catalog,
                None,
                &RebuildRequest::default(),
            )
        })
        .collect::<Vec<ExecutionPlan>>();
    // Per variant: operation id -> (fingerprint, dependency ids).
    let per_variant = plans
        .iter()
        .map(|plan| {
            plan.operations
                .iter()
                .map(|operation| {
                    (
                        operation.id.as_str().to_string(),
                        (
                            operation.fingerprint,
                            operation
                                .depends_on
                                .iter()
                                .map(|dependency| dependency.as_str().to_string())
                                .collect::<Vec<_>>(),
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();
    let mut kinds = BTreeMap::<String, OperationKind>::new();
    for plan in &plans {
        for operation in &plan.operations {
            kinds
                .entry(operation.id.as_str().to_string())
                .or_insert_with(|| operation.kind.clone());
        }
    }

    // Sources the shared Buildroot base reads: a direct dependency of an
    // image operation, or the image's own source. These cannot differ.
    let mut base_sources = BTreeSet::new();
    for spec in specs {
        match &spec.image.definition {
            gaia_spec::ImageDefinition::Buildroot(buildroot) => {
                base_sources.extend(buildroot.source.iter().map(|id| id.as_str().to_string()));
            }
            gaia_spec::ImageDefinition::StartingPoint(starting_point) => {
                base_sources.extend(
                    starting_point
                        .source
                        .iter()
                        .map(|id| id.as_str().to_string()),
                );
            }
        }
    }
    for (id, kind) in &kinds {
        if !matches!(
            kind,
            OperationKind::PrepareImage | OperationKind::BuildImage | OperationKind::AssembleImage
        ) {
            continue;
        }
        for map in &per_variant {
            if let Some((_, dependencies)) = map.get(id) {
                base_sources.extend(
                    dependencies
                        .iter()
                        .filter_map(|dependency| dependency.strip_prefix("source:"))
                        .map(str::to_string),
                );
            }
        }
    }

    let mut shared = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    let mut variant_sources = BTreeSet::new();
    let mut errors = Vec::new();
    for (id, kind) in &kinds {
        let entries = per_variant
            .iter()
            .map(|map| map.get(id))
            .collect::<Vec<_>>();
        let same = entries.iter().all(Option::is_some)
            && entries.windows(2).all(|pair| pair[0] == pair[1]);
        match kind {
            // Always executes; the shared state keeps one record of it.
            OperationKind::ResolveBuild => {
                shared.insert(id.clone());
            }
            // Per variant: these feed the image, which differs by variant.
            OperationKind::EmitReport
            | OperationKind::InstallArtifact { .. }
            | OperationKind::RenderStageFile { .. }
            | OperationKind::RenderStageEnvSet { .. }
            | OperationKind::RenderStageService { .. }
            | OperationKind::CaptureCheckpoint { .. }
            | OperationKind::BuildImage
            | OperationKind::AssembleImage => {}
            OperationKind::MaterializeSource { source_id } => {
                if same {
                    shared.insert(id.clone());
                } else if base_sources.contains(source_id.as_str()) {
                    errors.push(format!(
                        "the variants change source '{}', which the Buildroot base (image:prepare) or the image reads; variants cannot change the shared Buildroot base, so keep that source the same in every variant",
                        source_id.as_str()
                    ));
                } else {
                    variant_sources.insert(source_id.as_str().to_string());
                }
            }
            OperationKind::PrepareImage => {
                if same {
                    shared.insert(id.clone());
                } else {
                    errors.push(
                        "the variants change the Buildroot base (image:prepare); the base tree is shared, so variants may change only the image's feed and output".to_string(),
                    );
                }
            }
            OperationKind::BuildArtifact { .. } => {
                if same {
                    candidates.insert(id.clone());
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    // An artifact is shared only when everything it depends on is.
    let mut changed = true;
    while changed {
        changed = false;
        for id in candidates.clone() {
            let dependencies = &per_variant[0][&id].1;
            if dependencies
                .iter()
                .all(|dependency| shared.contains(dependency))
            {
                candidates.remove(&id);
                shared.insert(id);
                changed = true;
            }
        }
    }

    Ok(VariantSplit {
        shared,
        variant_sources,
        operation_counts: specs
            .iter()
            .zip(&plans)
            .map(|(spec, plan)| (variant_id(spec).to_string(), plan.operations.len()))
            .collect(),
    })
}

/// Points a variant's own outputs at its own paths. Shared outputs keep
/// their paths, so the variants reuse one copy of them. (Variant-specific
/// sources were scoped when the config was resolved for the variant.)
fn scope_variant(spec: &mut ResolvedBuildSpec, shared: &BTreeSet<String>) {
    let variant = variant_id(spec).to_string();
    for artifact in &mut spec.artifacts {
        let id = OperationId::artifact(&artifact.id);
        if !shared.contains(id.as_str()) {
            artifact.output.path = variant_output_path(&artifact.output.path, &variant);
        }
    }
    if spec.image.output.collect_dir.is_none() {
        let base = gaia_spec::default_image_collect_dir(&spec.workspace);
        spec.image.output.collect_dir = Some(format!("{}/{variant}", base.trim_end_matches('/')));
    }
}

/// `dir/file` becomes `dir/variants/<variant>/file`.
pub(crate) fn variant_output_path(path: &str, variant: &str) -> String {
    let path = Path::new(path);
    match path.file_name() {
        Some(file) => path
            .parent()
            .unwrap_or(Path::new(""))
            .join("variants")
            .join(variant)
            .join(file)
            .display()
            .to_string(),
        None => path.display().to_string(),
    }
}

/// The reuse state a build or variant reads before planning.
pub(crate) fn scoped_reuse_state(
    spec: &ResolvedBuildSpec,
    scope: ReuseScope<'_>,
) -> Option<ReuseState> {
    let ReuseScope::Variant { shared } = scope else {
        return load_reuse_state(spec);
    };
    let own = load_reuse_state(spec);
    let shared_state = load_reuse_state(&shared_store_spec(spec));
    if own.is_none() && shared_state.is_none() {
        return None;
    }
    let mut merged = own.unwrap_or_default();
    let shared_state = shared_state.unwrap_or_default();
    for id in shared {
        merged.completed_operation_ids.remove(id);
        merged.operation_fingerprints.remove(id);
        merged.operation_output_signatures.remove(id);
        merged.operation_input_signatures.remove(id);
        if shared_state.completed_operation_ids.contains(id) {
            merged.completed_operation_ids.insert(id.clone());
        }
        if let Some(value) = shared_state.operation_fingerprints.get(id) {
            merged.operation_fingerprints.insert(id.clone(), *value);
        }
        if let Some(value) = shared_state.operation_output_signatures.get(id) {
            merged
                .operation_output_signatures
                .insert(id.clone(), value.clone());
        }
        if let Some(value) = shared_state.operation_input_signatures.get(id) {
            merged.operation_input_signatures.insert(id.clone(), *value);
        }
    }
    Some(merged)
}

/// The spec whose state file is the shared store: the same build, without
/// a variant.
fn shared_store_spec(spec: &ResolvedBuildSpec) -> ResolvedBuildSpec {
    let mut shared = spec.clone();
    shared.selection.variant = None;
    shared
}

/// Records the shared operations of a variant's run in the shared store, so
/// the other variants reuse them.
pub(crate) fn save_shared_state(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
    shared: &BTreeSet<String>,
) {
    let shared_spec = shared_store_spec(spec);
    let previous = load_reuse_state(&shared_spec);
    let shared_plan = ExecutionPlan {
        build_id: plan.build_id.clone(),
        operations: plan
            .operations
            .iter()
            .filter(|operation| shared.contains(operation.id.as_str()))
            .cloned()
            .collect(),
    };
    save_reuse_state(&shared_spec, &shared_plan, outcome, previous.as_ref());
}

/// The indices of the variants to act on: all of them without a selection,
/// otherwise the named ones in file order.
pub(crate) fn selected_variants(
    specs: &[ResolvedBuildSpec],
    selected: &[String],
) -> Result<Vec<usize>, String> {
    let declared = specs.iter().map(variant_id).collect::<Vec<_>>();
    for name in selected {
        if !declared.contains(&name.as_str()) {
            return Err(format!(
                "unknown variant '{name}'; this build declares: {}",
                declared.join(", ")
            ));
        }
    }
    Ok(specs
        .iter()
        .enumerate()
        .filter(|(_, spec)| {
            selected.is_empty() || selected.iter().any(|name| name == variant_id(spec))
        })
        .map(|(index, _)| index)
        .collect())
}

fn split_summary(split: &VariantSplit) -> String {
    let parts = split
        .operation_counts
        .iter()
        .map(|(id, count)| {
            format!(
                "{id} {count} operation(s), {} variant-specific",
                count.saturating_sub(split.shared.len())
            )
        })
        .collect::<Vec<_>>();
    format!(
        "variants: {} shared operation(s); {}",
        split.shared.len(),
        parts.join("; ")
    )
}

/// `gaia run` for a build with variants: runs the selected variants in
/// order, stopping at the first that fails.
pub(crate) fn run_variants(
    context: &AppContext,
    source: VariantSource<'_>,
    specs: Vec<ResolvedBuildSpec>,
    selected: &[String],
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
) -> CommandOutcome {
    let indices = match selected_variants(&specs, selected) {
        Ok(indices) => indices,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let (specs, split) = match prepare_variants(context, source, specs) {
        Ok(prepared) => prepared,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let total = indices.len();
    let mut outcomes = Vec::new();
    for (position, index) in indices.into_iter().enumerate() {
        let spec = specs[index].clone();
        let id = variant_id(&spec).to_string();
        eprintln!("variant {id} ({}/{total})", position + 1);
        let outcome = run_outcome(collect_run_artifacts_for_spec(
            context,
            source.build,
            spec,
            targets,
            rebuild,
            ReuseScope::Variant {
                shared: &split.shared,
            },
        ));
        let stop = outcome.exit_code() != 0;
        outcomes.push(VariantOutcome { id, outcome });
        if stop {
            break;
        }
    }
    CommandOutcome::Variants {
        header: vec![split_summary(&split)],
        outcomes,
    }
}

/// `gaia preview` for a build with variants: one preview per selected
/// variant, each with the reuse state it would read.
pub(crate) fn preview_variants(
    context: &AppContext,
    source: VariantSource<'_>,
    selected: &[String],
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
    fail_on_clean: bool,
    json: bool,
) -> CommandOutcome {
    let specs = match resolve_variant_specs(source.build, source.options) {
        Ok(specs) => specs,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let indices = match selected_variants(&specs, selected) {
        Ok(indices) => indices,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let (specs, split) = match prepare_variants(context, source, specs) {
        Ok(prepared) => prepared,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let outcomes = indices
        .into_iter()
        .map(|index| {
            let spec = &specs[index];
            let id = variant_id(spec).to_string();
            let outcome = match super::preview::preview_resolved_scoped(
                context,
                spec,
                targets,
                rebuild,
                fail_on_clean,
                json,
                ReuseScope::Variant {
                    shared: &split.shared,
                },
            ) {
                Ok(report) => CommandOutcome::Previewed { report },
                Err(message) => CommandOutcome::Failed { message },
            };
            VariantOutcome { id, outcome }
        })
        .collect();
    CommandOutcome::Variants {
        header: vec![split_summary(&split)],
        outcomes,
    }
}

/// `gaia plan` for a build with variants.
pub(crate) fn plan_variants(
    context: &AppContext,
    source: VariantSource<'_>,
    specs: Vec<ResolvedBuildSpec>,
    selected: &[String],
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
) -> CommandOutcome {
    let indices = match selected_variants(&specs, selected) {
        Ok(indices) => indices,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let (specs, split) = match prepare_variants(context, source, specs) {
        Ok(prepared) => prepared,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let outcomes = indices
        .into_iter()
        .map(|index| {
            let spec = specs[index].clone();
            let id = variant_id(&spec).to_string();
            let outcome = super::plan::plan_resolved(
                context,
                spec,
                targets,
                rebuild,
                ReuseScope::Variant {
                    shared: &split.shared,
                },
            );
            VariantOutcome { id, outcome }
        })
        .collect();
    CommandOutcome::Variants {
        header: vec![split_summary(&split)],
        outcomes,
    }
}

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;
