//! Kernel modules step tests: the copied closure, builtins, missing modules,
//! the kernel version choice and depmod. Split from `tests.rs` for its length.

use super::*;
use gaia_spec::{AssemblyKernelModulesSpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;

fn host_has(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// `<images>/modules/<version>` as a kernel's module dir: a dependency
/// chain (usb_f_mass_storage -> libcomposite), an unrelated module, and a
/// builtin that has no module file.
fn fake_kernel_modules(root: &Path, version: &str) {
    let kernel = root.join("out/images/modules").join(version);
    let files = [
        ("kernel/usb/libcomposite.ko.xz", "composite"),
        ("kernel/usb/usb_f_mass_storage.ko", "mass storage"),
        ("kernel/net/unused.ko", "unused"),
    ];
    for (relative, contents) in files {
        let path = kernel.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("module dir");
        fs::write(path, contents).expect("module file");
    }
    fs::write(
        kernel.join("modules.dep"),
        "kernel/usb/libcomposite.ko.xz:\n\
         kernel/usb/usb_f_mass_storage.ko: kernel/usb/libcomposite.ko.xz\n\
         kernel/net/unused.ko:\n",
    )
    .expect("modules.dep");
    fs::write(kernel.join("modules.builtin"), "kernel/fs/vfat/vfat.ko\n").expect("modules.builtin");
    fs::write(kernel.join("modules.builtin.modinfo"), "vfat.license=GPL\n").expect("modinfo");
    fs::write(
        kernel.join("modules.order"),
        "kernel/usb/libcomposite.ko.xz\n",
    )
    .expect("order");
}

fn modules_assembly(
    root: &Path,
    kernel_version: Option<&str>,
    modules: &[&str],
    depmod: Option<&str>,
) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some(root.join("build/assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "initramfs".into(),
            path: "$assembly.work/initramfs".into(),
        }],
        kernel_modules: vec![AssemblyKernelModulesSpec {
            metadata: None,
            tree: "initramfs".into(),
            from: "$provider.images/modules".into(),
            kernel_version: kernel_version.map(str::to_string),
            modules: modules.iter().map(|name| name.to_string()).collect(),
            depmod: depmod.map(Into::into),
        }],
        ..ImageAssemblySpec::default()
    }
}

fn run_modules(spec: &ResolvedBuildSpec) -> Result<AssemblyStagingSummary, AssemblyError> {
    stage_image_assembly_with(spec, &OperationId::image_assembly(), None, || {
        panic!("a disk work dir reads no RAM facts")
    })
}

#[test]
fn kernel_modules_copy_the_dependency_closure_and_skip_builtins() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-modules");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(modules_assembly(
        &root,
        None,
        &["usb-f-mass-storage", "vfat"],
        None,
    ));

    let summary = run_modules(&spec).expect("kernel modules assembly");

    let target = root.join("build/assembly/initramfs/lib/modules/6.12.0");
    // The module and what it depends on, under the same relative paths.
    assert_eq!(
        fs::read_to_string(target.join("kernel/usb/usb_f_mass_storage.ko")).expect("mass storage"),
        "mass storage"
    );
    assert_eq!(
        fs::read_to_string(target.join("kernel/usb/libcomposite.ko.xz")).expect("composite"),
        "composite"
    );
    // Unrequested modules stay out; the builtin has no file to copy.
    assert!(!target.join("kernel/net/unused.ko").exists());
    assert!(!target.join("kernel/fs/vfat/vfat.ko").exists());
    for metadata in [
        "modules.builtin",
        "modules.builtin.modinfo",
        "modules.order",
    ] {
        assert!(target.join(metadata).is_file(), "missing {metadata}");
    }
    let state = summary.state.render();
    assert!(state.contains("kernel_modules.1.copied_count=2"), "{state}");
    assert!(
        state.contains("kernel_modules.1.builtin_count=1"),
        "{state}"
    );
    assert!(
        summary
            .messages
            .iter()
            .any(|message| message.contains("'vfat' is built into the kernel")),
        "{:?}",
        summary.messages
    );

    if host_has("depmod") {
        // depmod rewrites the index over exactly the copied set.
        let index =
            fs::read_to_string(target.join("modules.dep")).expect("regenerated modules.dep");
        assert!(index.contains("usb_f_mass_storage.ko"), "{index}");
        assert!(index.contains("libcomposite.ko.xz"), "{index}");
        assert!(!index.contains("unused.ko"), "{index}");
    }
}

#[test]
fn kernel_modules_name_a_module_that_exists_nowhere() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-missing");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(modules_assembly(&root, None, &["nope"], None));

    let error = run_modules(&spec).expect_err("missing module");

    assert!(
        error.message.contains("kernel module 'nope'"),
        "{}",
        error.message
    );
}

#[test]
fn kernel_modules_pick_the_only_version_or_the_named_one() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-versions");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    fake_kernel_modules(&root, "6.6.1");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(modules_assembly(&root, None, &["unused"], None));
    let error = run_modules(&spec).expect_err("two versions");
    assert!(
        error.message.contains("several kernel versions"),
        "{}",
        error.message
    );

    spec.image.assembly = Some(modules_assembly(&root, Some("6.6.1"), &["unused"], None));
    let summary = run_modules(&spec).expect("named version");
    assert!(
        summary
            .state
            .render()
            .contains("kernel_modules.1.kernel_version=6.6.1")
    );
    assert!(
        root.join("build/assembly/initramfs/lib/modules/6.6.1/kernel/net/unused.ko")
            .is_file()
    );
}

#[test]
fn kernel_modules_refuse_a_depmod_that_does_not_exist() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-depmod");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(modules_assembly(
        &root,
        None,
        &["unused"],
        Some("$provider.images/no-such-depmod"),
    ));

    let error = run_modules(&spec).expect_err("missing depmod");

    assert!(error.message.contains("depmod"), "{}", error.message);
    assert!(
        error.message.contains("does not exist"),
        "{}",
        error.message
    );
}

/// The module spec with a `metadata` list.
fn metadata_assembly(root: &Path, metadata: &[&str]) -> ImageAssemblySpec {
    let mut assembly = modules_assembly(root, None, &["usb-f-mass-storage"], None);
    assembly.kernel_modules[0].metadata =
        Some(metadata.iter().map(|name| name.to_string()).collect());
    assembly
}

#[test]
fn a_metadata_list_installs_only_the_listed_modules_files() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-metadata");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    let kernel = root.join("out/images/modules/6.12.0");
    fs::write(
        kernel.join("modules.alias"),
        "alias usb:* usb_f_mass_storage\n",
    )
    .expect("alias");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(metadata_assembly(&root, &["modules.dep", "modules.alias"]));

    run_modules(&spec).expect("kernel modules with a metadata list");

    let target = root.join("build/assembly/initramfs/lib/modules/6.12.0");
    assert!(target.join("modules.dep").is_file());
    assert!(target.join("modules.alias").is_file());
    let mut names: Vec<String> = fs::read_dir(&target)
        .expect("target")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("modules."))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["modules.alias", "modules.dep"],
        "only the listed files"
    );
    assert!(target.join("kernel/usb/usb_f_mass_storage.ko").is_file());
}

#[test]
fn a_listed_metadata_file_the_module_set_lacks_is_an_error() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-kernel-metadata-missing");
    fs::create_dir_all(&root).expect("test root");
    fake_kernel_modules(&root, "6.12.0");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(metadata_assembly(&root, &["modules.symbols"]));

    let error = run_modules(&spec).expect_err("missing listed metadata");

    assert!(
        error.message.contains("'modules.symbols'"),
        "{}",
        error.message
    );
}
