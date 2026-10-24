pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{AssemblyCheckSpec, ImageAssemblySpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;
use support::{assembly_file, assembly_tree, provider_catalogs, test_spec};

/// An assembly with one tree, `boot`, holding `config.txt`, and `checks`.
fn spec_with_checks(scratch: &TempDir, checks: Vec<AssemblyCheckSpec>) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let source = Path::new(&spec.workspace.build_dir).join("sources/config.txt");
    fs::create_dir_all(source.parent().expect("source dir")).expect("source dir");
    fs::write(&source, "dtoverlay=ov9782\n").expect("source");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("disk".into()),
        trees: vec![assembly_tree("boot", "$assembly.work/boot")],
        files: vec![assembly_file(
            "boot",
            source.display().to_string(),
            "config.txt",
        )],
        checks,
        ..ImageAssemblySpec::default()
    });
    spec
}

fn check(id: &str, command: &[&str]) -> AssemblyCheckSpec {
    AssemblyCheckSpec {
        id: id.into(),
        command: command.iter().map(|arg| arg.to_string()).collect(),
        timeout_secs: Some(30),
    }
}

/// Runs the assembly; the error messages, empty when it passed.
fn assemble(spec: &ResolvedBuildSpec) -> Vec<String> {
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    outcome
        .errors
        .iter()
        .map(|error| format!("{error:?}"))
        .collect()
}

#[test]
fn a_check_sees_the_finished_tree_through_its_token() {
    let scratch = TempDir::new("gaia-assembly-checks-pass");
    let spec = spec_with_checks(
        &scratch,
        vec![check(
            "boot-config",
            &[
                "grep",
                "-q",
                "dtoverlay=ov9782",
                "$assembly.tree.boot/config.txt",
            ],
        )],
    );
    let errors = assemble(&spec);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_failing_check_fails_the_assembly_with_its_output() {
    let scratch = TempDir::new("gaia-assembly-checks-fail");
    let spec = spec_with_checks(
        &scratch,
        vec![check(
            "boot-overlays",
            &[
                "sh",
                "-c",
                "echo 'overlays/ov9782.dtbo missing where the firmware reads' >&2; exit 3",
            ],
        )],
    );
    let errors = assemble(&spec).join("\n");
    assert!(
        errors.contains("assembly check 'boot-overlays' failed"),
        "{errors}"
    );
    assert!(
        errors.contains("missing where the firmware reads"),
        "{errors}"
    );
}
