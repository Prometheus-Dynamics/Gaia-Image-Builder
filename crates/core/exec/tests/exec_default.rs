pub mod support;

use gaia_exec::{ExecutionEvent, ExecutionProviders, execute_plan};
use gaia_plan::plan_build;
use std::fs;
use std::path::Path;
use support::{provider_catalogs, test_spec};

#[test]
fn executes_default_plan_with_runtime_events() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
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

    assert_eq!(outcome.completed_operations, 11);
    assert!(outcome.errors.is_empty());
    assert!(outcome.reused_ids.is_empty());
    assert!(
        outcome
            .events
            .iter()
            .any(|event| matches!(event, ExecutionEvent::Started { .. }))
    );
    assert!(
        outcome
            .events
            .iter()
            .any(|event| matches!(event, ExecutionEvent::Succeeded { .. }))
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream/source.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream/.gaia-source-state.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/workspace-root/source.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/workspace-root/.gaia-source-state.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("artifacts/gaia")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("artifacts/.gaia/gaia.gaia-state.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("images/image-provider.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("images/.gaia-image-state.txt")
            .is_file()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("images/default-2.0.0.tar")
            .is_file()
    );
    let runtime_dir = Path::new(&spec.workspace.out_dir).join(".gaia/runtime");
    assert!(
        fs::read_to_string(runtime_dir.join("install-install-gaia-app.state"))
            .expect("install runtime state")
            .contains("dest=/usr/bin/default")
    );
    assert!(
        fs::read_to_string(runtime_dir.join("stage-file-motd.state"))
            .expect("stage file runtime state")
            .contains("dest=/etc/motd")
    );
    assert!(
        fs::read_to_string(runtime_dir.join("stage-file-motd.state"))
            .expect("stage file runtime state")
            .contains("origin=static-asset")
    );
    assert!(
        fs::read_to_string(runtime_dir.join("checkpoint-base-image.state"))
            .expect("checkpoint runtime state")
            .contains("backend=local")
    );
    assert!(
        fs::read_to_string(runtime_dir.join("checkpoint-base-image.state"))
            .expect("checkpoint runtime state")
            .contains("anchor=image")
    );
}
