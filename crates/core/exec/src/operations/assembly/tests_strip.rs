//! `strip = true` on files, binaries' copies and trees, and the strip tool
//! choices. A real ELF is the host's own test binary, which has symbols.
//! Split from `tests.rs` for its length.

use super::tests::test_spec as base_spec;
use super::tests_binaries::{disk_env_panic, state_value};
use super::*;
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

/// The test spec with a command timeout that a strip of a test binary fits in.
fn strip_spec(root: &Path) -> ResolvedBuildSpec {
    let mut spec = base_spec(root);
    spec.policy.providers.buildroot.timeout_seconds = 300;
    spec.policy.providers.starting_point.timeout_seconds = 300;
    spec
}

fn host_has(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn file_entry(src: &Path, dest: &str, strip: bool) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: Vec::new(),
        strip,
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

fn assembly(tree: &Path, tree_strip: bool, files: Vec<AssemblyFileSpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: tree.display().to_string().into(),
            strip: tree_strip,
        }],
        files,
        ..ImageAssemblySpec::default()
    }
}

fn stage(spec: &ResolvedBuildSpec) -> AssemblyStagingSummary {
    stage_image_assembly_with(spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("assembly with strip")
}

/// A `cpio` image of the tree, so the tree is packed (and stripped) first.
fn packed(assembly: &mut ImageAssemblySpec) {
    assembly
        .filesystems
        .push(gaia_spec::AssemblyFilesystemSpec {
            free: None,
            id: "rootfs".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::Cpio,
            source_tree: "rootfs".into(),
            output: "$assembly.work/rootfs.cpio".into(),
            size: None,
            deterministic: true,
            compression_level: None,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            compressed: Default::default(),
            fat_size: None,
            reserved_percent: None,
        });
}

#[cfg(unix)]
#[test]
fn stripped_file_copy_is_smaller_and_the_source_is_untouched() {
    if !host_has("strip") {
        eprintln!("skipping: no strip on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-exec-strip-file");
    let root = scratch.join("gaia-strip-file");
    fs::create_dir_all(&root).expect("test root");
    let source = std::env::current_exe().expect("test binary");
    let source_bytes = fs::read(&source).expect("source");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    spec.image.assembly = Some(assembly(
        &tree,
        false,
        vec![file_entry(&source, "bin/tool", true)],
    ));

    let summary = stage(&spec);

    let copy = fs::read(tree.join("bin/tool")).expect("copy");
    assert!(
        copy.len() < source_bytes.len(),
        "{} vs {}",
        copy.len(),
        source_bytes.len()
    );
    let sections = gaia_plan::elf_sections(&copy)
        .expect("readable")
        .expect("ELF");
    assert!(!sections.has_symtab && !sections.has_debug);
    assert_eq!(fs::read(&source).expect("source again"), source_bytes);
    assert_eq!(state_value(&summary, "stripped_file_count"), "1");
}

#[cfg(unix)]
#[test]
fn a_non_elf_file_with_strip_is_copied_unchanged() {
    let scratch = TempDir::new("gaia-exec-strip-text");
    let root = scratch.join("gaia-strip-text");
    fs::create_dir_all(&root).expect("test root");
    let source = root.join("notes.txt");
    fs::write(&source, "plain text, not ELF\n").expect("source");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    spec.image.assembly = Some(assembly(
        &tree,
        false,
        vec![file_entry(&source, "notes.txt", true)],
    ));

    let summary = stage(&spec);

    assert_eq!(
        fs::read_to_string(tree.join("notes.txt")).expect("copy"),
        "plain text, not ELF\n"
    );
    assert!(!summary.state.render().contains("stripped_file_count"));
}

#[cfg(unix)]
#[test]
fn a_strip_tool_path_that_does_not_exist_fails_naming_it() {
    let scratch = TempDir::new("gaia-exec-strip-missing");
    let root = scratch.join("gaia-strip-missing");
    fs::create_dir_all(&root).expect("test root");
    let source = std::env::current_exe().expect("test binary");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    let mut assembly = assembly(&tree, false, vec![file_entry(&source, "bin/tool", true)]);
    assembly.strip_tool = Some("/nonexistent/gaia-strip".into());
    spec.image.assembly = Some(assembly);

    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("missing strip tool");

    assert!(error.message.contains("strip_tool"), "{}", error.message);
    assert!(
        error.message.contains("/nonexistent/gaia-strip"),
        "{}",
        error.message
    );
}

#[cfg(unix)]
#[test]
fn a_packed_tree_with_strip_is_stripped_before_it_is_packed() {
    if !host_has("strip") {
        eprintln!("skipping: no strip on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-exec-strip-tree");
    let root = scratch.join("gaia-strip-tree");
    fs::create_dir_all(&root).expect("test root");
    let source = std::env::current_exe().expect("test binary");
    let source_len = fs::metadata(&source).expect("source").len();
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    let mut assembly = assembly(&tree, true, vec![file_entry(&source, "bin/tool", false)]);
    packed(&mut assembly);
    spec.image.assembly = Some(assembly);

    let summary = stage(&spec);

    let stripped_len = fs::metadata(tree.join("bin/tool")).expect("copy").len();
    assert!(stripped_len < source_len);
    assert_eq!(state_value(&summary, "strip.1.tree"), "rootfs");
    assert_eq!(state_value(&summary, "strip.1.files"), "1");
    let saved: u64 = state_value(&summary, "strip.1.saved_bytes")
        .parse()
        .expect("bytes");
    assert_eq!(saved, source_len - stripped_len);
    assert!(
        summary
            .messages
            .iter()
            .any(|line| line.contains("stripped 1 file(s) in tree 'rootfs'"))
    );
}

#[test]
fn a_file_read_by_seeking_gives_the_same_sections_as_its_bytes() {
    let source = std::env::current_exe().expect("test binary");
    let bytes = fs::read(&source).expect("bytes");
    let by_bytes = gaia_plan::elf_sections(&bytes).expect("readable");
    let mut file = fs::File::open(&source).expect("open");
    let by_seek = gaia_plan::elf_sections_of_file(&mut file).expect("readable");
    assert!(by_bytes.is_some());
    assert_eq!(by_seek, by_bytes);
    let scratch = TempDir::new("gaia-exec-strip-seek");
    let plain = scratch.join("plain.txt");
    fs::write(&plain, "not an ELF file\n").expect("text");
    let mut text = fs::File::open(&plain).expect("open");
    assert_eq!(gaia_plan::elf_sections_of_file(&mut text), Ok(None));
}

#[cfg(unix)]
#[test]
fn many_stripped_files_are_batched_and_their_totals_add_up() {
    if !host_has("strip") || !host_has("cc") {
        eprintln!("skipping: no strip or cc on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-exec-strip-batch");
    let root = scratch.join("gaia-strip-batch");
    fs::create_dir_all(&root).expect("test root");
    let source = root.join("unit.c");
    fs::write(&source, "int unit_value(int x) { return x * 2; }\n").expect("source");
    let object = root.join("unit.o");
    let built = std::process::Command::new("cc")
        .args(["-c", "-g"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .status()
        .expect("cc");
    assert!(built.success());
    let object_len = fs::metadata(&object).expect("object").len();
    // More files than one strip run takes, so several runs share the work.
    let copies = 150;
    let files: Vec<AssemblyFileSpec> = (0..copies)
        .map(|index| file_entry(&object, &format!("bin/unit{index:03}.o"), false))
        .collect();
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    let mut assembly = assembly(&tree, true, files);
    packed(&mut assembly);
    spec.image.assembly = Some(assembly);

    let summary = stage(&spec);

    assert_eq!(state_value(&summary, "strip.1.files"), copies.to_string());
    let saved: u64 = state_value(&summary, "strip.1.saved_bytes")
        .parse()
        .expect("bytes");
    let mut left = 0u64;
    for index in 0..copies {
        let copy = tree.join(format!("bin/unit{index:03}.o"));
        left += fs::metadata(&copy).expect("copy").len();
        let bytes = fs::read(&copy).expect("read copy");
        let sections = gaia_plan::elf_sections(&bytes)
            .expect("readable")
            .expect("ELF");
        assert!(!sections.has_debug, "{}", copy.display());
    }
    assert_eq!(saved, copies * object_len - left);
    assert!(saved > 0);
}

#[cfg(unix)]
#[test]
fn kernel_modules_keep_their_symbols_and_lose_only_debug_data() {
    if !host_has("strip") {
        eprintln!("skipping: no strip on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-exec-strip-ko");
    let root = scratch.join("gaia-strip-ko");
    fs::create_dir_all(&root).expect("test root");
    let source = std::env::current_exe().expect("test binary");
    let source_len = fs::metadata(&source).expect("source").len();
    let tree = root.join("build/assembly/rootfs");
    let mut spec = strip_spec(&root);
    let mut assembly = assembly(&tree, true, vec![file_entry(&source, "lib/x.ko", false)]);
    packed(&mut assembly);
    spec.image.assembly = Some(assembly);

    stage(&spec);

    let module = fs::read(tree.join("lib/x.ko")).expect("module");
    assert!((module.len() as u64) < source_len);
    let sections = gaia_plan::elf_sections(&module)
        .expect("readable")
        .expect("ELF");
    assert!(sections.has_symtab, "--strip-debug keeps the symbol table");
    assert!(!sections.has_debug);
}

#[cfg(unix)]
#[test]
fn hard_links_stay_linked_and_symlinks_are_left_alone_when_stripping() {
    if !host_has("strip") {
        eprintln!("skipping: no strip on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-exec-strip-links");
    let root = scratch.join("gaia-strip-links");
    let tree = root.join("tree");
    fs::create_dir_all(tree.join("bin")).expect("tree");
    let source = std::env::current_exe().expect("test binary");
    fs::copy(&source, tree.join("bin/real")).expect("copy");
    fs::hard_link(tree.join("bin/real"), tree.join("bin/alias")).expect("hard link");
    std::os::unix::fs::symlink("real", tree.join("bin/link")).expect("symlink");
    let mut spec = strip_spec(&root);
    spec.image.assembly = Some(assembly(&tree, false, Vec::new()));
    let roots =
        AssemblyRoots::new(&spec, spec.image.assembly.as_ref().expect("assembly")).expect("roots");
    let configured = None;
    let mut stripper = Stripper::new(&spec, &roots, configured, None);
    let files = vec![
        tree.join("bin/real"),
        tree.join("bin/alias"),
        tree.join("bin/link"),
    ];

    let summary = stripper.strip_paths(&files).expect("strip");

    assert_eq!(summary.files, 1, "one inode, stripped once");
    use std::os::unix::fs::MetadataExt;
    let real = fs::metadata(tree.join("bin/real")).expect("real");
    let alias = fs::metadata(tree.join("bin/alias")).expect("alias");
    assert_eq!((real.dev(), real.ino()), (alias.dev(), alias.ino()));
    assert!(real.len() < fs::metadata(&source).expect("source").len());
    assert!(
        fs::symlink_metadata(tree.join("bin/link"))
            .expect("link")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(tree.join("bin/link")).expect("target"),
        Path::new("real")
    );
}
