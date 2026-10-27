//! The concurrent assembly steps: a run's state and messages do not depend on
//! how its steps were scheduled, and the first failing step in step order is
//! the one reported. Split from `tests.rs`.

use super::tests::test_spec;
use super::tests_binaries::disk_env_panic;
use super::*;
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

fn file(src: &Path, tree: &str, dest: &str) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        exclude: Vec::new(),
        tree: tree.into(),
        src: Some(src.display().to_string().into()),
        src_glob: None,
        content: None,
        json: None,
        dest: dest.into(),
        mode: None,
        dir_mode: None,
        optional: false,
        src_dir: None,
        preserve_symlink: false,
        allow_missing_libs: false,
    }
}

fn three_trees(files: Vec<AssemblyFileSpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: ["a", "b", "c"]
            .iter()
            .map(|id| AssemblyTreeSpec {
                id: (*id).into(),
                path: format!("$assembly.work/tree-{id}").into(),
                strip: false,
            })
            .collect(),
        files,
        ..ImageAssemblySpec::default()
    }
}

/// The messages that are not step timings: those depend on the clock.
fn stable_messages(summary: &AssemblyStagingSummary) -> Vec<String> {
    summary
        .messages
        .iter()
        .filter(|line| gaia_process::parse_step_time(line).is_none())
        .cloned()
        .collect()
}

#[test]
fn independent_steps_record_the_same_state_and_messages_on_every_run() {
    let scratch = TempDir::new("gaia-exec-schedule-same");
    let root = scratch.join("gaia-schedule-same");
    fs::create_dir_all(root.join("src")).expect("src");
    let mut files = Vec::new();
    for tree in ["a", "b", "c"] {
        for index in 0..4 {
            let src = root.join(format!("src/{tree}{index}.txt"));
            fs::write(&src, format!("{tree} {index}\n")).expect("source");
            files.push(file(&src, tree, &format!("f{index}.txt")));
        }
    }
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(three_trees(files));

    let first =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("first run");
    let second =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("second run");

    assert_eq!(first.state, second.state);
    assert_eq!(stable_messages(&first), stable_messages(&second));
    assert_eq!(
        super::tests_binaries::state_value(&first, "staged_file_count"),
        "12"
    );
    // Every file reached its tree.
    for tree in ["a", "b", "c"] {
        for index in 0..4 {
            let copied = root.join(format!("build/assembly/tree-{tree}/f{index}.txt"));
            assert_eq!(
                fs::read_to_string(&copied).expect("copy"),
                format!("{tree} {index}\n")
            );
        }
    }
}

#[test]
fn the_first_failing_step_in_step_order_is_the_error() {
    let scratch = TempDir::new("gaia-exec-schedule-error");
    let root = scratch.join("gaia-schedule-error");
    fs::create_dir_all(root.join("src")).expect("src");
    let present = root.join("src/present.txt");
    fs::write(&present, "present\n").expect("present");
    // The first entry declared fails; a later one fails too, and may fail
    // first in time. The error is the first one in step order.
    let files = vec![
        file(&root.join("src/missing-a.txt"), "a", "missing-a.txt"),
        file(&present, "b", "present.txt"),
        file(&root.join("src/missing-b.txt"), "b", "missing-b.txt"),
    ];
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(three_trees(files));

    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("a missing source fails the run");

    assert!(error.message.contains("missing-a.txt"), "{}", error.message);
    assert!(
        !error.message.contains("missing-b.txt"),
        "{}",
        error.message
    );
}
