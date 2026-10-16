//! The runtime-library guard for copied files: a `[[image.assembly.files]]`
//! entry that copies a dynamically linked program warns when its tree lacks
//! the libraries it needs. Split from `tests.rs` for its length.

use super::tests_binaries::{
    binary, disk_env_panic, state_value, write_dynamic_tool, write_target_libraries,
};
use super::*;
use gaia_plan::test_fixtures::{EM_X86_64, ElfFixture, build_elf};
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

fn file_entry(src: &Path, dest: &str) -> AssemblyFileSpec {
    AssemblyFileSpec {
        tree: "rootfs".into(),
        src: Some(src.display().to_string().into()),
        src_glob: None,
        exclude: Vec::new(),
        content: None,
        json: None,
        dest: dest.into(),
        mode: None,
        optional: false,
        preserve_symlink: false,
        allow_missing_libs: false,
    }
}

fn files_assembly(tree: &Path, files: Vec<AssemblyFileSpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: tree.display().to_string().into(),
        }],
        files,
        ..ImageAssemblySpec::default()
    }
}

fn stage(spec: &ResolvedBuildSpec) -> AssemblyStagingSummary {
    stage_image_assembly_with(spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("file assembly")
}

#[cfg(unix)]
#[test]
fn a_dynamic_file_copied_into_a_tree_without_its_libraries_warns() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-missing");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(files_assembly(
        &tree,
        vec![file_entry(&tool, "usr/sbin/tool")],
    ));

    let summary = stage(&spec);

    assert_eq!(summary.warnings.len(), 1, "{:?}", summary.warnings);
    let warning = &summary.warnings[0];
    assert!(warning.contains("'/usr/sbin/tool'"), "{warning}");
    assert!(warning.contains("in tree 'rootfs'"), "{warning}");
    assert!(warning.contains("libc.so.6 (needed by"), "{warning}");
    assert!(
        warning.contains("/lib/ld-test.so.1 (program interpreter)"),
        "{warning}"
    );
    assert!(warning.contains("[[image.assembly.binaries]]"), "{warning}");
    assert!(
        summary
            .messages
            .iter()
            .any(|message| message == &format!("warning: {warning}")),
        "{:?}",
        summary.messages
    );
    assert!(state_value(&summary, "runtime_lib_warning.1").contains("libc.so.6"));
}

#[cfg(unix)]
#[test]
fn a_dynamic_file_whose_libraries_the_tree_holds_does_not_warn() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-present");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(files_assembly(
        &tree,
        vec![
            file_entry(&tool, "usr/sbin/tool"),
            file_entry(&root.join("target/lib/ld-test-real.so"), "lib/ld-test.so.1"),
            file_entry(&root.join("target/lib/libc-2.0.so"), "lib/libc.so.6"),
        ],
    ));

    let summary = stage(&spec);

    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
}

#[cfg(unix)]
#[test]
fn libraries_brought_by_a_later_binaries_entry_do_not_warn() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-binaries");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    let mut assembly = files_assembly(&tree, vec![file_entry(&tool, "usr/sbin/tool")]);
    assembly.binaries = vec![binary(&tool, "usr/sbin/tool")];
    spec.image.assembly = Some(assembly);

    let summary = stage(&spec);

    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
    assert!(tree.join("lib/libc.so.6").exists());
}

#[cfg(unix)]
#[test]
fn allow_missing_libs_silences_the_warning() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-allowed");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut entry = file_entry(&tool, "usr/sbin/tool");
    entry.allow_missing_libs = true;
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(files_assembly(&tree, vec![entry]));

    let summary = stage(&spec);

    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
}

#[cfg(unix)]
#[test]
fn static_and_non_elf_files_never_warn() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-static");
    fs::create_dir_all(&root).expect("test root");
    let static_tool = root.join("static-tool");
    fs::write(
        &static_tool,
        build_elf(&ElfFixture::dynamic(EM_X86_64, &[])),
    )
    .expect("static fixture");
    let text = root.join("motd.txt");
    fs::write(&text, b"hello\n").expect("text fixture");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(files_assembly(
        &tree,
        vec![
            file_entry(&static_tool, "usr/sbin/static-tool"),
            file_entry(&text, "etc/motd"),
        ],
    ));

    let summary = stage(&spec);

    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
    assert!(
        summary
            .messages
            .iter()
            .all(|message| !message.starts_with("warning:")),
        "{:?}",
        summary.messages
    );
}
