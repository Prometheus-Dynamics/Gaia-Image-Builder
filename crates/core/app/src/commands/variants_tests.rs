//! Tests of the variant classification, scoping and selection.

use super::*;
fn variant_build(out_dir: &std::path::Path) -> String {
    format!(
        r#"
build_name = "pv"
version = "1.0.0"

[inputs.pv]
kind = "string"
default = "2027"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "{out}"

[[sources]]
id = "app-source"
kind = "path"
path = "app"

[[artifacts]]
id = "jar"
kind = "rust"
package = "app-${{input.pv}}"
source = "app-source"
profile = "release"
target = "x86_64-unknown-linux-musl"
install_name = "app"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/app.bin"

[image]
kind = "starting-point"
rootfs_path = "seed/base.img"

[[variants]]
id = "pv2026"
overrides = [["input.pv", "2026"]]
"#,
        root = out_dir.join("root").display(),
        out = out_dir.display(),
    )
}

fn resolved_variants(name: &str) -> (gaia_testkit::TempDir, Vec<ResolvedBuildSpec>) {
    let scratch = gaia_testkit::TempDir::new(&format!("gaia-variants-{name}"));
    let dir = scratch.path().to_path_buf();
    let build = dir.join("pv.toml");
    std::fs::write(&build, variant_build(&dir.join("out"))).expect("build file");
    let specs = gaia_config::resolve_variant_specs(
        build.to_str().expect("utf-8 path"),
        &gaia_config::ResolveOptions::default(),
    )
    .expect("variants resolve");
    (scratch, specs)
}

#[test]
fn the_source_is_shared_and_the_artifact_built_per_variant() {
    let (_dir, mut specs) = resolved_variants("classify");
    let context = AppContext::with_defaults();
    let split = classify(&context, &specs).expect("variants classify");
    let source = OperationId::source(&specs[0].sources[0].id);
    let artifact = OperationId::artifact(&specs[0].artifacts[0].id);
    assert!(split.shared.contains(source.as_str()), "{:?}", split.shared);
    assert!(
        !split.shared.contains(artifact.as_str()),
        "{:?}",
        split.shared
    );
    assert!(split.shared.contains(OperationId::resolve().as_str()));
    assert_eq!(split.operation_counts.len(), 2);

    // Only the artifact's own output moves: each variant gets its own file.
    let before = specs[0].artifacts[0].output.path.clone();
    for spec in &mut specs {
        scope_variant(spec, &split.shared);
    }
    assert_eq!(
        specs[0].artifacts[0].output.path,
        variant_output_path(&before, "default")
    );
    assert_ne!(
        specs[0].artifacts[0].output.path,
        specs[1].artifacts[0].output.path
    );
    assert!(
        specs[1].artifacts[0]
            .output
            .path
            .contains("variants/pv2026")
    );
    assert_ne!(
        specs[0].image.output.collect_dir,
        specs[1].image.output.collect_dir
    );
    assert!(variant_output_collisions(&specs).is_empty());
}

#[test]
fn a_shared_operation_recorded_by_one_variant_is_reused_by_the_next() {
    let (scratch, mut specs) = resolved_variants("handoff");
    let context = AppContext::with_defaults();
    let split = classify(&context, &specs).expect("classify");
    for spec in &mut specs {
        scope_variant(spec, &split.shared);
    }
    let default = &specs[0];
    let plan = plan_build_with_rebuilds(
        default,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        None,
        &RebuildRequest::default(),
    );
    let source = OperationId::source(&default.sources[0].id);
    let outcome = ExecutionOutcome {
        completed_ids: vec![source.clone()],
        ..ExecutionOutcome::default()
    };
    // The default variant ran first and recorded the shared source.
    save_shared_state(default, &plan, &outcome, &split.shared);
    assert!(scratch.join("out/.gaia/pv.reuse-state").is_file());

    let scope = ReuseScope::Variant {
        shared: &split.shared,
    };
    let state = scoped_reuse_state(&specs[1], scope).expect("shared state for pv2026");
    assert!(state.completed_operation_ids.contains(source.as_str()));
    assert_eq!(
        state.operation_fingerprints.get(source.as_str()),
        default_fingerprint(&plan, &source).as_ref()
    );
    // The variant's own state does not exist yet, so nothing else is recorded.
    assert!(
        !state
            .completed_operation_ids
            .contains(OperationId::artifact(&specs[1].artifacts[0].id).as_str())
    );
}

fn default_fingerprint(plan: &ExecutionPlan, id: &OperationId) -> Option<u64> {
    plan.operations
        .iter()
        .find(|operation| &operation.id == id)
        .map(|operation| operation.fingerprint)
}

/// A build whose base (the starting-point image) reads `base-rootfs`, and
/// whose artifact reads `app-source`. Each variant moves one path source.
fn base_source_build(out_dir: &std::path::Path, variant_overrides: &str) -> String {
    format!(
        r#"
build_name = "pv"
version = "1.0.0"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "{out}"

[[sources]]
id = "app-source"
kind = "path"
path = "app"

[[sources]]
id = "base-rootfs"
kind = "path"
path = "seed"

[[artifacts]]
id = "jar"
kind = "rust"
package = "app"
source = "app-source"
profile = "release"
target = "x86_64-unknown-linux-musl"
install_name = "app"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/app.bin"

[image]
kind = "starting-point"
source = "base-rootfs"
rootfs_path = "seed"

[[variants]]
id = "moved"
overrides = [{variant_overrides}]
"#,
        root = out_dir.join("root").display(),
        out = out_dir.display(),
    )
}

fn classify_build(name: &str, variant_overrides: &str) -> Result<VariantSplit, String> {
    let scratch = gaia_testkit::TempDir::new(&format!("gaia-variants-{name}"));
    let dir = scratch.path().to_path_buf();
    let build = dir.join("pv.toml");
    std::fs::write(
        &build,
        base_source_build(&dir.join("out"), variant_overrides),
    )
    .expect("build file");
    let specs = gaia_config::resolve_variant_specs(
        build.to_str().expect("utf-8 path"),
        &gaia_config::ResolveOptions::default(),
    )
    .expect("variants resolve");
    classify(&AppContext::with_defaults(), &specs)
}

#[test]
fn a_source_the_buildroot_base_reads_cannot_differ_between_variants() {
    let error = classify_build(
        "base-source",
        r#"["sources.base-rootfs.path", "other-seed"]"#,
    )
    .expect_err("the base source must not differ");
    assert!(
        error.contains("'base-rootfs'")
            && error.contains("cannot change the shared Buildroot base"),
        "{error}"
    );
}

#[test]
fn a_source_only_an_artifact_reads_may_differ_between_variants() {
    let split = classify_build("artifact-source", r#"["sources.app-source.path", "app2"]"#)
        .expect("classifies");
    assert!(split.variant_sources.contains("app-source"), "{split:?}");
    assert!(!split.variant_sources.contains("base-rootfs"), "{split:?}");
    assert!(
        split
            .shared
            .contains(OperationId::source(&"base-rootfs".into()).as_str()),
        "{split:?}"
    );
}

#[test]
fn variant_outputs_move_into_the_variant_directory() {
    assert_eq!(
        variant_output_path("/b/artifacts/jar.jar", "pv2026"),
        "/b/artifacts/variants/pv2026/jar.jar"
    );
    assert_eq!(
        variant_output_path("out/x.bin", "default"),
        "out/variants/default/x.bin"
    );
}

#[test]
fn selection_names_declared_variants_only() {
    let specs = ["default", "pv2026", "pv2027"]
        .iter()
        .map(|id| {
            let mut spec = ResolvedBuildSpec::new("pv");
            spec.selection.variant = Some((*id).to_string());
            spec
        })
        .collect::<Vec<_>>();
    assert_eq!(selected_variants(&specs, &[]).unwrap(), [0, 1, 2]);
    assert_eq!(
        selected_variants(&specs, &["pv2027".to_string()]).unwrap(),
        [2]
    );
    let error = selected_variants(&specs, &["pv2030".to_string()]).unwrap_err();
    assert!(
        error.contains("declares: default, pv2026, pv2027"),
        "{error}"
    );
}
