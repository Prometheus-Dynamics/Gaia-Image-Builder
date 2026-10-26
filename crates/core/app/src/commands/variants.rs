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

use gaia_config::{VariantBuild, variant_output_collisions};
use gaia_exec::ExecutionOutcome;
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, PlanTarget, RebuildRequest, ReuseState,
    plan_build_operations,
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

/// The build a variant command acts on, loaded once for all its variants.
#[derive(Clone, Copy)]
pub(crate) struct VariantSource<'a> {
    pub(crate) build: &'a VariantBuild,
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
/// reported before anything runs. The variant build resolves the specs again
/// once it is known which sources each variant checks out itself, from the
/// same load.
pub(crate) fn prepare_variants(
    context: &AppContext,
    source: VariantSource<'_>,
    mut specs: Vec<ResolvedBuildSpec>,
) -> Result<PreparedVariants, String> {
    // Every declared variant must be valid, whatever `--variant` selects:
    // which operations are shared is decided across all of them.
    let mut errors = Vec::new();
    for spec in specs.iter() {
        let validation = validate_spec_with_providers(
            spec,
            &context.source_catalog,
            &context.artifact_catalog,
            &context.image_catalog,
        );
        errors.extend(
            validation
                .errors
                .iter()
                .map(|error| format!("variant {}: {error}", variant_id(spec))),
        );
    }
    if !errors.is_empty() {
        return Err(format!(
            "build '{}' has {} validation error(s) in its variants (every variant must be valid, since what they share is decided across all of them):\n{}",
            specs[0].display_name(),
            errors.len(),
            errors.join("\n")
        ));
    }
    let (split, plans) = classify_planned(context, &specs)?;
    let classified = specs.clone();
    if !split.variant_sources.is_empty() {
        specs = source
            .build
            .resolve_scoped(&split.variant_sources)
            .map_err(|error| error.to_string())?;
    }
    for spec in specs.iter_mut() {
        scope_variant(spec, &split.shared);
    }
    let collisions = variant_output_collisions(&specs);
    if !collisions.is_empty() {
        return Err(collisions.join("\n"));
    }
    // A plan made for the classified spec serves the final one where the two
    // are equal, and is planned again where scoping changed the spec.
    let structures = plans
        .into_iter()
        .zip(classified.iter().zip(&specs))
        .map(|(plan, (before, after))| (before == after).then_some(plan))
        .collect();
    Ok(PreparedVariants {
        specs,
        split,
        structures,
    })
}

/// The validated, classified and scoped variants of a build.
pub(crate) struct PreparedVariants {
    pub(crate) specs: Vec<ResolvedBuildSpec>,
    pub(crate) split: VariantSplit,
    /// Per spec: its plan of the structure (see
    /// `gaia_plan::plan_build_operations`), where that plan is still the one
    /// the spec would make, so a run or plan decides reuse on it.
    pub(crate) structures: Vec<Option<ExecutionPlan>>,
}

/// Plans every variant without reuse state and splits its operations.
#[cfg(test)]
fn classify(context: &AppContext, specs: &[ResolvedBuildSpec]) -> Result<VariantSplit, String> {
    classify_planned(context, specs).map(|(split, _)| split)
}

/// [`classify`], which also returns each spec's plan of the structure, so
/// the variants can be decided against their reuse state without planning
/// them again.
fn classify_planned(
    context: &AppContext,
    specs: &[ResolvedBuildSpec],
) -> Result<(VariantSplit, Vec<ExecutionPlan>), String> {
    let plans = specs
        .iter()
        .map(|spec| {
            plan_build_operations(
                spec,
                &context.source_catalog,
                &context.artifact_catalog,
                &context.image_catalog,
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

    // Sources the shared Buildroot base reads: a direct dependency of
    // image:prepare (always planned with variants), or the image's own
    // source. These cannot differ. image:build and image:assembly run per
    // variant, so sources only they read (an assembly naming
    // `${source.<id>.commit_short}`) may differ like any artifact's.
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
            gaia_spec::ImageDefinition::None => {}
        }
    }
    for (id, kind) in &kinds {
        if !matches!(kind, OperationKind::PrepareImage) {
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
            | OperationKind::AssembleImage
            | OperationKind::CheckImage => {}
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
                    errors.push(format!(
                        "the variants change the Buildroot base (image:prepare); the base tree is shared, so variants may change only the image's feed and output: {}",
                        operation_differences(specs, &plans, id)
                    ));
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

    let split = VariantSplit {
        shared,
        variant_sources,
        operation_counts: specs
            .iter()
            .zip(&plans)
            .map(|(spec, plan)| (variant_id(spec).to_string(), plan.operations.len()))
            .collect(),
    };
    Ok((split, plans))
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
    // Entries of two fingerprint schemes cannot share one scheme: a mix
    // re-evaluates everything once (scheme 0 matches no current scheme).
    let fingerprint_scheme = match (
        own.as_ref().map(|state| state.fingerprint_scheme),
        shared_state.as_ref().map(|state| state.fingerprint_scheme),
    ) {
        (Some(own), Some(shared)) if own != shared => 0,
        (Some(scheme), _) | (None, Some(scheme)) => scheme,
        (None, None) => 0,
    };
    let mut merged = own.unwrap_or_default();
    merged.fingerprint_scheme = fingerprint_scheme;
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
pub(crate) fn shared_store_spec(spec: &ResolvedBuildSpec) -> ResolvedBuildSpec {
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
    // Each variant's plan below lists every operation it has, shared ones
    // included (run once, by the first variant, then reused): name them.
    let shared = split
        .shared
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "variants: {} shared operation(s); {}\nshared (run once, reused by the other variants): {shared}",
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
    let PreparedVariants {
        specs,
        split,
        mut structures,
    } = match prepare_variants(context, source, specs) {
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
            source.build.build(),
            spec,
            targets,
            rebuild,
            ReuseScope::Variant {
                shared: &split.shared,
            },
            structures[index].take(),
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
    let specs = match source.build.resolve() {
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
    let PreparedVariants { specs, split, .. } = match prepare_variants(context, source, specs) {
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
    let PreparedVariants {
        specs,
        split,
        structures,
    } = match prepare_variants(context, source, specs) {
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
                structures[index].clone(),
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

/// What differs in operation `id` between the first variant and each other
/// one: its named fingerprint inputs and its dependencies.
fn operation_differences(specs: &[ResolvedBuildSpec], plans: &[ExecutionPlan], id: &str) -> String {
    let find = |plan: &ExecutionPlan| {
        plan.operations
            .iter()
            .find(|operation| operation.id.as_str() == id)
            .cloned()
    };
    let (Some(base_spec), Some(base_plan)) = (specs.first(), plans.first()) else {
        return String::new();
    };
    let Some(base) = find(base_plan) else {
        return String::new();
    };
    let base_parts = gaia_plan::operation_components(base_spec, base_plan, &base)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let base_deps = base
        .depends_on
        .iter()
        .map(|dependency| dependency.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let mut out = Vec::new();
    for (spec, plan) in specs.iter().zip(plans).skip(1) {
        let Some(operation) = find(plan) else {
            out.push(format!("variant {} has no {id}", variant_id(spec)));
            continue;
        };
        let parts = gaia_plan::operation_components(spec, plan, &operation)
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut changed = parts
            .iter()
            .filter(|(name, digest)| base_parts.get(*name) != Some(*digest))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        changed.extend(
            base_parts
                .keys()
                .filter(|name| !parts.contains_key(*name))
                .map(|name| format!("{name} (removed)")),
        );
        let deps = operation
            .depends_on
            .iter()
            .map(|dependency| dependency.as_str().to_string())
            .collect::<BTreeSet<_>>();
        let added = deps.difference(&base_deps).cloned().collect::<Vec<_>>();
        let removed = base_deps.difference(&deps).cloned().collect::<Vec<_>>();
        let mut items = Vec::new();
        if operation.fingerprint != base.fingerprint {
            if changed.is_empty() {
                items.push("its fingerprint (no named input differs)".to_string());
            } else {
                items.push(format!("inputs {}", changed.join(", ")));
            }
        }
        if !added.is_empty() {
            items.push(format!("added dependencies {}", added.join(", ")));
        }
        if !removed.is_empty() {
            items.push(format!("removed dependencies {}", removed.join(", ")));
        }
        if items.is_empty() && operation.depends_on != base.depends_on {
            items.push("the order of its dependencies".to_string());
        }
        if items.is_empty() {
            continue;
        }
        out.push(format!(
            "variant {} differs in {}",
            variant_id(spec),
            items.join("; ")
        ));
    }
    out.join("; ")
}
