//! Listed binaries (`[[image.assembly.binaries]]`): a binary copied into a
//! tree with its runtime closure. Split from `tests.rs` for its length.

use super::*;
use gaia_plan::test_fixtures::{EM_X86_64, ElfFixture, build_elf};
use gaia_spec::{AssemblyBinarySpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

/// A binary built for the target, in the read-only target tree of a Buildroot
/// build. The test that reads it is skipped when it is absent.
const TARGET_E2FSCK: &str =
    "/dev/shm/gaia-sozo/217f87a2dbce/buildroot-output/target/usr/sbin/e2fsck";

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, bytes).expect("fixture file");
}

#[cfg(unix)]
fn symlink(target: &str, path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    std::os::unix::fs::symlink(target, path).expect("fixture symlink");
}

/// A target root's loader and C library, as the sysroot of the tools below.
fn write_target_libraries(root: &Path) {
    let sysroot = root.join("target");
    write(
        &sysroot.join("lib/ld-test-real.so"),
        &build_elf(&ElfFixture::dynamic(EM_X86_64, &[])),
    );
    symlink("ld-test-real.so", &sysroot.join("lib/ld-test.so.1"));
    write(
        &sysroot.join("lib/libc-2.0.so"),
        &build_elf(&ElfFixture::dynamic(EM_X86_64, &[])),
    );
    symlink("libc-2.0.so", &sysroot.join("lib/libc.so.6"));
}

/// A dynamic `usr/sbin/<tool>` in the target root that needs both libraries.
/// Returns the tool's path.
fn write_dynamic_tool(root: &Path, tool: &str) -> PathBuf {
    let path = root.join("target/usr/sbin").join(tool);
    write(
        &path,
        &build_elf(&ElfFixture {
            interpreter: Some("/lib/ld-test.so.1"),
            ..ElfFixture::dynamic(EM_X86_64, &["libc.so.6"])
        }),
    );
    path
}

fn binaries_assembly(tree: &Path, binaries: Vec<AssemblyBinarySpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: tree.display().to_string().into(),
        }],
        binaries,
        ..ImageAssemblySpec::default()
    }
}

fn binary(src: &Path, dest: &str) -> AssemblyBinarySpec {
    AssemblyBinarySpec {
        tree: "rootfs".into(),
        src: src.display().to_string().into(),
        dest: dest.into(),
        sysroot: None,
        mode: None,
    }
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

#[cfg(unix)]
#[test]
fn a_dynamic_binary_is_copied_with_its_loader_and_libraries() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-exec-binaries");
    let root = scratch.join("gaia-binaries-dynamic");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let tool = write_dynamic_tool(&root, "tool");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(binaries_assembly(
        &tree,
        vec![binary(&tool, "usr/sbin/tool")],
    ));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("binary assembly");

    let copied = tree.join("usr/sbin/tool");
    assert_eq!(
        fs::read(&copied).expect("tool"),
        fs::read(&tool).expect("source")
    );
    assert_eq!(
        fs::metadata(&copied).expect("tool").permissions().mode() & 0o7777,
        0o755
    );
    assert_eq!(
        fs::read(tree.join("lib/ld-test.so.1")).expect("loader through its link"),
        fs::read(root.join("target/lib/ld-test-real.so")).expect("loader")
    );
    assert_eq!(
        fs::read(tree.join("lib/libc.so.6")).expect("libc through its link"),
        fs::read(root.join("target/lib/libc-2.0.so")).expect("libc")
    );
    assert_eq!(state_value(&summary, "binary.1.runtime_linkage"), "dynamic");
    assert_eq!(state_value(&summary, "completed_binary_count"), "1");
}

#[cfg(unix)]
#[test]
fn a_static_binary_is_only_copied_and_the_mode_is_taken_from_the_config() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-exec-binaries");
    let root = scratch.join("gaia-binaries-static");
    fs::create_dir_all(&root).expect("test root");
    let tool = root.join("static/bin/tool");
    // An object with no interpreter and no needed libraries is static.
    write(&tool, &build_elf(&ElfFixture::dynamic(EM_X86_64, &[])));
    let tree = root.join("build/assembly/rootfs");
    let mut entry = binary(&tool, "bin/tool");
    entry.mode = Some("0700".into());
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(binaries_assembly(&tree, vec![entry]));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("static binary assembly");

    assert_eq!(
        fs::metadata(tree.join("bin/tool"))
            .expect("tool")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    assert_eq!(state_value(&summary, "binary.1.runtime_linkage"), "static");
    assert_eq!(state_value(&summary, "binary.1.runtime_library_count"), "0");
    assert!(
        !tree.join("lib").exists(),
        "a static binary needs no libraries"
    );
}

#[cfg(unix)]
#[test]
fn binaries_sharing_libraries_copy_each_library_once_without_conflict() {
    let scratch = TempDir::new("gaia-exec-binaries");
    let root = scratch.join("gaia-binaries-shared");
    fs::create_dir_all(&root).expect("test root");
    write_target_libraries(&root);
    let first = write_dynamic_tool(&root, "first");
    let second = write_dynamic_tool(&root, "second");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(binaries_assembly(
        &tree,
        vec![
            binary(&first, "usr/sbin/first"),
            binary(&second, "usr/sbin/second"),
        ],
    ));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("two binaries assembly");

    assert!(tree.join("usr/sbin/first").is_file());
    assert!(tree.join("usr/sbin/second").is_file());
    assert_eq!(
        fs::read(tree.join("lib/libc.so.6")).expect("shared libc"),
        fs::read(root.join("target/lib/libc-2.0.so")).expect("libc")
    );
    assert_eq!(state_value(&summary, "completed_binary_count"), "2");
    assert_eq!(state_value(&summary, "binary.2.runtime_linkage"), "dynamic");
}

#[test]
fn a_missing_binary_and_an_escaping_dest_are_errors() {
    let scratch = TempDir::new("gaia-exec-binaries");
    let root = scratch.join("gaia-binaries-errors");
    fs::create_dir_all(&root).expect("test root");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);

    spec.image.assembly = Some(binaries_assembly(
        &tree,
        vec![binary(&root.join("nope/tool"), "usr/bin/tool")],
    ));
    let missing =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("missing binary");
    assert!(
        missing.message.contains("does not exist"),
        "{}",
        missing.message
    );

    let tool = root.join("static/tool");
    write(&tool, &build_elf(&ElfFixture::dynamic(EM_X86_64, &[])));
    spec.image.assembly = Some(binaries_assembly(&tree, vec![binary(&tool, "../escape")]));
    let escaping =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("escaping dest");
    assert!(
        escaping.message.contains("escapes tree"),
        "{}",
        escaping.message
    );
}

/// The real target's `e2fsck` and the loader and C library it names, when the
/// Buildroot target tree is present. Skipped otherwise.
#[cfg(unix)]
#[test]
fn target_e2fsck_brings_the_loader_and_libc_of_its_sysroot() {
    let src = Path::new(TARGET_E2FSCK);
    if !src.is_file() {
        eprintln!("skipped: {TARGET_E2FSCK} is not present");
        return;
    }
    let scratch = TempDir::new("gaia-exec-binaries");
    let root = scratch.join("gaia-binaries-target-e2fsck");
    fs::create_dir_all(&root).expect("test root");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(binaries_assembly(
        &tree,
        vec![binary(src, "usr/sbin/e2fsck")],
    ));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("e2fsck assembly");

    let closure = gaia_plan::resolve_runtime_closure(src, None).expect("closure");
    assert!(closure.dynamic);
    let interpreter = closure.interpreter.expect("interpreter");
    let copied_loader = tree.join(interpreter.trim_start_matches('/'));
    assert!(
        fs::read(&copied_loader).expect("loader in the tree")[..4] == *b"\x7fELF",
        "{}",
        copied_loader.display()
    );
    assert_eq!(state_value(&summary, "binary.1.runtime_linkage"), "dynamic");
}
