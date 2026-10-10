pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationKind, OperationReuse, ReuseState, operation_input_signature,
    operation_output_signature, plan_build, plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{AssemblyArchiveSpec, AssemblyPathTemplate, ImageAssemblySpec, ResolvedBuildSpec};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use support::{provider_catalogs, test_spec};

const REUSABLE_IDS: [&str; 11] = [
    "source:gaia-upstream",
    "source:workspace-root",
    "artifact:gaia-app",
    "install:install-gaia-app",
    "stage:file:motd",
    "stage:env:runtime-env",
    "stage:service:gaia-service",
    "image:build",
    "image:assembly",
    "checkpoint:base-image",
    "image:prepare",
];

/// The provider's state file as one run leaves it. Each run rewrites it with
/// different digests, `reused` and archive bookkeeping, so it is never an
/// input to the signatures under test.
fn provider_state(run: u32) -> String {
    format!(
        "provider=image.buildroot\nbackend_mode=buildroot\ncollect_digest=run{run}\narchive_sha256=run{run}\nreused={}\nemit_report=true\n",
        run > 1
    )
}

fn write_collected_images(collect: &Path, rootfs: &str) {
    fs::create_dir_all(collect).expect("collect dir");
    fs::write(
        collect.join("image-provider.txt"),
        "provider=image.buildroot\n",
    )
    .expect("provider marker");
    fs::write(collect.join("rootfs.img"), rootfs).expect("rootfs image");
    fs::write(collect.join("archive.tar"), "archive-bytes").expect("archive");
}

/// Sets a file's modification time, the way a rebuild or a restore does
/// without touching the bytes.
fn set_mtime(path: &Path, seconds: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime")
        .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
        .expect("set mtime");
}

struct Fixture {
    spec: ResolvedBuildSpec,
    collect: PathBuf,
    assembly_output: PathBuf,
}

fn fixture() -> Fixture {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out_dir = PathBuf::from(&spec.workspace.out_dir);
    let collect = PathBuf::from(spec.image.output.collect_dir.clone().expect("collect dir"));
    let assembly_output = build_dir.join("assembly/rootfs-archive.tar");
    spec.image.assembly = Some(ImageAssemblySpec {
        archives: vec![AssemblyArchiveSpec {
            id: "rootfs-archive".into(),
            output: AssemblyPathTemplate::new(assembly_output.display().to_string()),
            members: Vec::new(),
        }],
        ..ImageAssemblySpec::default()
    });

    for dir in [
        build_dir.join("sources/gaia-upstream"),
        build_dir.join("sources/workspace-root"),
        build_dir.join("image/buildroot-output/target"),
        out_dir.join(".gaia/runtime"),
    ] {
        fs::create_dir_all(dir).expect("fixture dir");
    }
    for source in ["gaia-upstream", "workspace-root"] {
        fs::write(build_dir.join(format!("sources/{source}/source.txt")), "ok")
            .expect("source marker");
    }
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    write_collected_images(&collect, "image-v1");
    fs::write(collect.join(".gaia-image-state.txt"), provider_state(1)).expect("state");
    fs::create_dir_all(assembly_output.parent().expect("assembly dir")).expect("assembly dir");
    fs::write(&assembly_output, "assembly-v1").expect("assembly output");

    let runtime = out_dir.join(".gaia/runtime");
    fs::write(
        runtime.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\n",
    )
    .expect("install state");
    fs::write(
        runtime.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\n",
    )
    .expect("stage file state");
    fs::write(
        runtime.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\n",
    )
    .expect("stage env state");
    fs::write(
        runtime.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\n",
    )
    .expect("stage service state");
    fs::write(
        runtime.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\n",
    )
    .expect("checkpoint state");
    fs::write(
        runtime.join("image-assembly.state"),
        "kind=image-assembly\n",
    )
    .expect("assembly runtime state");

    Fixture {
        spec,
        collect,
        assembly_output,
    }
}

fn build_image_signature(spec: &ResolvedBuildSpec) -> Option<String> {
    operation_output_signature(spec, &OperationKind::BuildImage)
}

/// A reuse state recording every reusable operation of `baseline`, with the
/// input signatures a completed run would have written.
fn full_state(spec: &ResolvedBuildSpec, baseline: &ExecutionPlan) -> ReuseState {
    let recorded = baseline
        .operations
        .iter()
        .filter(|operation| REUSABLE_IDS.contains(&operation.id.as_str()));
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: recorded
            .clone()
            .map(|operation| operation.id.as_str().to_string())
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: recorded
            .clone()
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: recorded
            .clone()
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: recorded
            .map(|operation| {
                (
                    operation.id.as_str().to_string(),
                    operation_input_signature(spec, baseline, operation),
                )
            })
            .collect(),
    }
}

fn reuse_of<'plan>(plan: &'plan ExecutionPlan, id: &str) -> &'plan OperationReuse {
    &plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
        .reuse
}

fn replan(spec: &ResolvedBuildSpec, state: &ReuseState) -> ExecutionPlan {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(state),
    )
}

#[test]
fn build_image_signature_ignores_rewritten_provider_state_and_mtimes() {
    let fixture = fixture();
    let collect = &fixture.collect;
    let first = build_image_signature(&fixture.spec).expect("build image signature");

    // The next identical build rewrites the state file and the image files'
    // mtimes, but the bytes are the same.
    fs::write(collect.join(".gaia-image-state.txt"), provider_state(2)).expect("rewrite state");
    set_mtime(&collect.join("rootfs.img"), 1_700_000_000);
    set_mtime(&collect.join("archive.tar"), 1_700_000_100);
    let second = build_image_signature(&fixture.spec).expect("build image signature");

    assert_eq!(first, second);
}

#[test]
fn build_image_signature_changes_when_an_image_byte_changes() {
    let fixture = fixture();
    let collect = &fixture.collect;
    let first = build_image_signature(&fixture.spec).expect("build image signature");

    // Same length, one byte different, and a different mtime.
    fs::write(collect.join("rootfs.img"), "image-v2").expect("changed image");
    set_mtime(&collect.join("rootfs.img"), 1_700_000_200);
    let second = build_image_signature(&fixture.spec).expect("build image signature");

    assert_ne!(first, second);
    assert!(!second.contains(collect.to_str().expect("utf-8 path")));
}

#[test]
fn identical_image_rebuild_keeps_assembly_reused() {
    let fixture = fixture();
    let spec = &fixture.spec;
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state = full_state(spec, &baseline);

    // The rebuild rewrites the provider state and mtimes with identical bytes.
    fs::write(
        fixture.collect.join(".gaia-image-state.txt"),
        provider_state(2),
    )
    .expect("rewrite state");
    set_mtime(&fixture.collect.join("rootfs.img"), 1_700_000_000);
    set_mtime(&fixture.collect.join("archive.tar"), 1_700_000_100);

    let plan = replan(spec, &state);
    assert!(
        matches!(reuse_of(&plan, "image:build"), OperationReuse::Reuse { .. }),
        "image:build should be reused: {:?}",
        reuse_of(&plan, "image:build")
    );
    assert!(
        matches!(
            reuse_of(&plan, "image:assembly"),
            OperationReuse::Reuse { .. }
        ),
        "image:assembly should be reused: {:?}",
        reuse_of(&plan, "image:assembly")
    );
}

#[test]
fn changed_image_byte_reruns_assembly() {
    let fixture = fixture();
    let spec = &fixture.spec;
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state = full_state(spec, &baseline);

    fs::write(fixture.collect.join("rootfs.img"), "image-v2").expect("changed image");
    set_mtime(&fixture.collect.join("rootfs.img"), 1_700_000_200);

    let plan = replan(spec, &state);
    assert!(
        matches!(
            reuse_of(&plan, "image:assembly"),
            OperationReuse::Execute(_)
        ),
        "a changed image must rerun assembly: {:?}",
        reuse_of(&plan, "image:assembly")
    );
}

#[test]
fn assembly_reruns_only_when_its_output_bytes_change() {
    let fixture = fixture();
    let spec = &fixture.spec;
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state = full_state(spec, &baseline);

    // Identical bytes, new mtime: still reused.
    fs::write(&fixture.assembly_output, "assembly-v1").expect("same assembly bytes");
    set_mtime(&fixture.assembly_output, 1_700_000_300);
    let plan = replan(spec, &state);
    assert!(
        matches!(
            reuse_of(&plan, "image:assembly"),
            OperationReuse::Reuse { .. }
        ),
        "identical assembly output must stay reused: {:?}",
        reuse_of(&plan, "image:assembly")
    );

    // Changed bytes: the output signature no longer matches.
    fs::write(&fixture.assembly_output, "assembly-v2").expect("changed assembly bytes");
    let plan = replan(spec, &state);
    assert!(
        matches!(
            reuse_of(&plan, "image:assembly"),
            OperationReuse::Execute(reason) if reason.code == "operation_output_changed"
        ),
        "changed assembly output must rerun: {:?}",
        reuse_of(&plan, "image:assembly")
    );
}
