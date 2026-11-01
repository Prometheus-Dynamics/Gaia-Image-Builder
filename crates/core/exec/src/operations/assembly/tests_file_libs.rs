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
        include: Vec::new(),
        strip: false,
        tree: "rootfs".into(),
        src: Some(src.display().to_string().into()),
        src_glob: None,
        exclude: Vec::new(),
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

fn files_assembly(tree: &Path, files: Vec<AssemblyFileSpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
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

#[cfg(unix)]
#[test]
fn an_app_layer_tree_above_the_image_layers_finds_their_libraries() {
    let scratch = TempDir::new("gaia-exec-file-libs");
    let root = scratch.join("gaia-file-libs-layers");
    fs::create_dir_all(&root).expect("test root");
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    let base = root.join("base.tar");
    // An empty tar archive: two zero blocks.
    fs::write(&base, [0u8; 1024]).expect("base tar");
    let mut assembly = files_assembly(&tree, vec![file_entry(&tool, "usr/bin/tool")]);
    assembly.ocis = vec![gaia_spec::AssemblyOciSpec {
        id: "sim".into(),
        dest: root.join("sim.oci").display().to_string().into(),
        format: "oci-layout".into(),
        layers: vec![
            gaia_spec::AssemblyOciLayerSpec {
                tar: Some(base.display().to_string().into()),
                tree: None,
            },
            gaia_spec::AssemblyOciLayerSpec {
                tar: None,
                tree: Some("rootfs".into()),
            },
        ],
        compression: "none".into(),
        reference: None,
        config: gaia_spec::AssemblyOciConfigSpec {
            architecture: "amd64".into(),
            os: "linux".into(),
            ..Default::default()
        },
    }];
    let target = gaia_spec::AssemblyRoots::new(&spec, &assembly)
        .expect("roots")
        .provider_target;
    spec.image.assembly = Some(assembly);

    // Without the image's root filesystem, the tree's own libraries are all
    // there is: the program's interpreter and libc are missing.
    let summary = stage(&spec);
    assert_eq!(summary.warnings.len(), 1, "{:?}", summary.warnings);
    assert!(
        summary.warnings[0].contains("libc.so.6"),
        "{:?}",
        summary.warnings
    );

    // With them in the root filesystem the layers come from, they are found.
    write_target_libraries(target.parent().expect("target parent"));
    assert_eq!(
        target.file_name().and_then(|name| name.to_str()),
        Some("target")
    );
    let summary = stage(&spec);
    assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
}
