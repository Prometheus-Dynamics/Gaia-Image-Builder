//! The device checks of `[image.assembly.packages.device]` on synthetic ELF
//! files: a sysroot with a C library that defines glibc versions, and a package
//! tree whose files each break (or keep) one of the checks.

use super::super::package_device::check_device;
use super::tests_binaries::write;
use gaia_plan::test_fixtures::{EM_AARCH64, EM_X86_64, ElfFixture, ElfVersions, build_elf_with};
use gaia_spec::{
    AssemblyPackageControlSpec, AssemblyPackageDeviceSpec, AssemblyPackageScriptsSpec,
    AssemblyPackageSpec, ResolvedBuildSpec, ToolchainKindSpec, ToolchainSpec,
};
use gaia_testkit::TempDir;
use std::path::{Path, PathBuf};

/// A device sysroot under `root`: its C library defines GLIBC_2.17 to 2.41 and
/// a private version that the default limit must ignore.
fn write_sysroot(root: &Path) -> PathBuf {
    let sysroot = root.join("sysroot");
    let libc = build_elf_with(
        &aarch64(&[]),
        &ElfVersions {
            needs: &[],
            defs: &["GLIBC_2.17", "GLIBC_2.34", "GLIBC_2.41", "GLIBC_PRIVATE"],
            load_align: 0x10000,
        },
    );
    write(&sysroot.join("lib/libc.so.6"), &libc);
    sysroot
}

fn aarch64<'a>(needed: &'a [&'a str]) -> ElfFixture<'a> {
    ElfFixture {
        is_64bit: true,
        little_endian: true,
        machine: EM_AARCH64,
        interpreter: None,
        needed,
        runpath: None,
    }
}

fn device(sysroot: &Path) -> AssemblyPackageDeviceSpec {
    AssemblyPackageDeviceSpec {
        toolchain: None,
        sysroot: Some(sysroot.display().to_string()),
        max_glibc: None,
        min_page_size: Some(16384),
        check_needed: true,
    }
}

fn package(device: AssemblyPackageDeviceSpec) -> AssemblyPackageSpec {
    AssemblyPackageSpec {
        id: "tool-pkg".into(),
        format: "ipk".into(),
        tree: "pkg-root".into(),
        dest: "out/".into(),
        output: "out/tool.ipk".into(),
        debian_binary: true,
        allowed_prefixes: Vec::new(),
        control: AssemblyPackageControlSpec::default(),
        scripts: AssemblyPackageScriptsSpec::default(),
        device: Some(device),
    }
}

/// A tool that needs `libc.so.6` at `version`, with `load_align` for its loads.
fn tool(needs_version: &str, load_align: u64) -> Vec<u8> {
    build_elf_with(
        &aarch64(&["libc.so.6"]),
        &ElfVersions {
            needs: &[("libc.so.6", needs_version)],
            defs: &[],
            load_align,
        },
    )
}

#[test]
fn a_package_that_fits_the_device_passes_every_check() {
    let scratch = TempDir::new("gaia-device-pass");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x10000));
    let spec = ResolvedBuildSpec::new("device");

    let check = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .expect("a fitting package passes");
    assert_eq!(check.elf_files, 1);
}

#[test]
fn a_glibc_version_above_max_glibc_names_the_file_and_the_version() {
    let scratch = TempDir::new("gaia-device-glibc");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.34", 0x10000));
    let spec = ResolvedBuildSpec::new("device");
    let mut limited = device(&sysroot);
    limited.max_glibc = Some("2.17".into());

    let error = check_device(&spec, &package(limited.clone()), &limited, &tree)
        .err()
        .expect("GLIBC_2.34 is above 2.17");
    assert!(error.contains("/usr/bin/tool"), "{error}");
    assert!(error.contains("GLIBC_2.34 from libc.so.6"), "{error}");
    assert!(error.contains("max_glibc 2.17"), "{error}");
}

#[test]
fn without_max_glibc_the_sysroot_libc_sets_the_limit() {
    let scratch = TempDir::new("gaia-device-default-max");
    let sysroot = write_sysroot(&scratch.join("root"));
    let spec = ResolvedBuildSpec::new("device");

    let fits = scratch.join("fits");
    write(&fits.join("usr/bin/tool"), &tool("GLIBC_2.41", 0x10000));
    check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &fits)
        .expect("GLIBC_2.41 is the sysroot's highest version");

    let too_new = scratch.join("too-new");
    write(&too_new.join("usr/bin/tool"), &tool("GLIBC_2.42", 0x10000));
    let error = check_device(
        &spec,
        &package(device(&sysroot)),
        &device(&sysroot),
        &too_new,
    )
    .err()
    .expect("GLIBC_2.42 is above the sysroot's highest version");
    assert!(error.contains("GLIBC_2.42"), "{error}");
    assert!(error.contains("max_glibc 2.41"), "{error}");
}

#[test]
fn a_segment_aligned_below_min_page_size_is_named_with_its_alignment() {
    let scratch = TempDir::new("gaia-device-align");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x1000));
    let spec = ResolvedBuildSpec::new("device");

    let error = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .err()
        .expect("a 4 KiB segment fails a 16 KiB device");
    assert!(error.contains("/usr/bin/tool"), "{error}");
    assert!(error.contains("p_align 0x1000"), "{error}");
    assert!(error.contains("min_page_size 0x4000"), "{error}");
}

#[test]
fn an_x86_64_file_in_an_aarch64_package_is_a_wrong_architecture() {
    let scratch = TempDir::new("gaia-device-arch");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(
        &tree.join("usr/bin/host-tool"),
        &build_elf_with(
            &ElfFixture {
                machine: EM_X86_64,
                ..aarch64(&[])
            },
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    let spec = ResolvedBuildSpec::new("device");

    let error = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .err()
        .expect("an x86-64 file is not for an AArch64 device");
    assert!(error.contains("/usr/bin/host-tool"), "{error}");
    assert!(error.contains("x86-64"), "{error}");
    assert!(error.contains("AArch64"), "{error}");
}

#[test]
fn a_needed_library_in_neither_the_sysroot_nor_the_package_is_named() {
    let scratch = TempDir::new("gaia-device-missing");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(
        &tree.join("usr/bin/tool"),
        &build_elf_with(
            &aarch64(&["libnowhere.so.1"]),
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    let spec = ResolvedBuildSpec::new("device");

    let error = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .err()
        .expect("nothing provides libnowhere.so.1");
    assert!(error.contains("libnowhere.so.1"), "{error}");
    assert!(error.contains("/usr/bin/tool"), "{error}");
}

#[test]
fn a_library_the_package_ships_is_found_through_runpath_origin() {
    let scratch = TempDir::new("gaia-device-runpath");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(
        &tree.join("usr/lib/libmine.so.1"),
        &build_elf_with(
            &aarch64(&[]),
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    write(
        &tree.join("usr/bin/tool"),
        &build_elf_with(
            &ElfFixture {
                runpath: Some("$ORIGIN/../lib"),
                ..aarch64(&["libmine.so.1"])
            },
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    let spec = ResolvedBuildSpec::new("device");

    let check = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .expect("the package's own library is found through $ORIGIN");
    assert_eq!(check.elf_files, 2);
}

#[test]
fn check_needed_false_leaves_the_libraries_unchecked() {
    let scratch = TempDir::new("gaia-device-no-needed");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(
        &tree.join("usr/bin/tool"),
        &build_elf_with(
            &aarch64(&["libnowhere.so.1"]),
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    let spec = ResolvedBuildSpec::new("device");
    let mut unchecked = device(&sysroot);
    unchecked.check_needed = false;

    check_device(&spec, &package(unchecked.clone()), &unchecked, &tree)
        .expect("DT_NEEDED is not checked");
}

#[test]
fn a_toolchain_names_the_sysroot_it_checks_against() {
    let scratch = TempDir::new("gaia-device-toolchain");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x10000));
    let mut spec = ResolvedBuildSpec::new("device");
    spec.toolchains.push(ToolchainSpec {
        id: "sdk".into(),
        kind: ToolchainKindSpec::BuildrootSdk,
        root_source: None,
        root_path: Some(scratch.join("root").display().to_string()),
        relocate: false,
        prefix: None,
        triple: "aarch64-buildroot-linux-gnu".into(),
        sysroot_path: Some("sysroot".into()),
        sysroot_source: None,
        rust_target: None,
        page_size: None,
    });
    let by_toolchain = AssemblyPackageDeviceSpec {
        toolchain: Some("sdk".into()),
        sysroot: None,
        ..device(&sysroot)
    };

    check_device(&spec, &package(by_toolchain.clone()), &by_toolchain, &tree)
        .expect("the toolchain's sysroot is the device");
}

/// A spec whose toolchain `sdk` is the sysroot under `root`, with `page_size`.
fn spec_with_sdk(root: &Path, page_size: Option<u64>) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("device");
    spec.toolchains.push(ToolchainSpec {
        id: "sdk".into(),
        kind: ToolchainKindSpec::BuildrootSdk,
        root_source: None,
        root_path: Some(root.display().to_string()),
        relocate: false,
        prefix: None,
        triple: "aarch64-buildroot-linux-gnu".into(),
        sysroot_path: Some("sysroot".into()),
        sysroot_source: None,
        rust_target: None,
        page_size,
    });
    spec
}

/// The device table naming the `sdk` toolchain, with `min_page_size`.
fn sdk_device(min_page_size: Option<u64>) -> AssemblyPackageDeviceSpec {
    AssemblyPackageDeviceSpec {
        toolchain: Some("sdk".into()),
        sysroot: None,
        min_page_size,
        ..device(Path::new("unused"))
    }
}

#[test]
fn the_toolchains_page_size_is_the_minimum_when_the_device_names_no_minimum() {
    let scratch = TempDir::new("gaia-device-page-size");
    let root = scratch.join("root");
    write_sysroot(&root);
    let spec = spec_with_sdk(&root, Some(16384));
    let device = sdk_device(None);

    let aligned = scratch.join("aligned");
    write(&aligned.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x4000));
    check_device(&spec, &package(device.clone()), &device, &aligned)
        .expect("16 KiB segments meet a 16 KiB page size");

    let small = scratch.join("small");
    write(&small.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x1000));
    let error = check_device(&spec, &package(device.clone()), &device, &small)
        .err()
        .expect("4 KiB segments are below the page size");
    assert!(error.contains("p_align 0x1000"), "{error}");
    assert!(error.contains("min_page_size 0x4000"), "{error}");
}

#[test]
fn without_a_page_size_or_a_minimum_the_default_is_4_kib() {
    let scratch = TempDir::new("gaia-device-default-4k");
    let root = scratch.join("root");
    write_sysroot(&root);
    let spec = spec_with_sdk(&root, None);
    let device = sdk_device(None);
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x1000));
    check_device(&spec, &package(device.clone()), &device, &tree)
        .expect("4 KiB segments meet the 4 KiB default");
}

#[test]
fn an_explicit_min_page_size_wins_over_the_toolchains_page_size() {
    let scratch = TempDir::new("gaia-device-explicit-min");
    let root = scratch.join("root");
    write_sysroot(&root);
    let spec = spec_with_sdk(&root, Some(4096));
    let device = sdk_device(Some(16384));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x1000));
    let error = check_device(&spec, &package(device.clone()), &device, &tree)
        .err()
        .expect("the explicit 16 KiB minimum applies");
    assert!(error.contains("min_page_size 0x4000"), "{error}");
}

#[test]
fn a_sysroot_without_a_c_library_is_reported() {
    let scratch = TempDir::new("gaia-device-no-libc");
    let sysroot = scratch.join("sysroot");
    std::fs::create_dir_all(sysroot.join("lib")).expect("sysroot");
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x10000));
    let spec = ResolvedBuildSpec::new("device");

    let error = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .err()
        .expect("no libc.so.6 in the sysroot");
    assert!(error.contains("has no libc.so.6"), "{error}");
}

#[test]
fn a_missing_sysroot_directory_is_reported() {
    let scratch = TempDir::new("gaia-device-no-sysroot");
    let sysroot = scratch.join("absent");
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/tool"), &tool("GLIBC_2.17", 0x10000));
    let spec = ResolvedBuildSpec::new("device");

    let error = check_device(&spec, &package(device(&sysroot)), &device(&sysroot), &tree)
        .err()
        .expect("the sysroot does not exist");
    assert!(error.contains("is not a directory"), "{error}");
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let scratch = TempDir::new("gaia-device-all");
    let sysroot = write_sysroot(&scratch.join("root"));
    let tree = scratch.join("pkg");
    write(&tree.join("usr/bin/a"), &tool("GLIBC_2.34", 0x1000));
    write(
        &tree.join("usr/bin/b"),
        &build_elf_with(
            &ElfFixture {
                machine: EM_X86_64,
                ..aarch64(&[])
            },
            &ElfVersions {
                load_align: 0x10000,
                ..ElfVersions::default()
            },
        ),
    );
    let spec = ResolvedBuildSpec::new("device");
    let mut limited = device(&sysroot);
    limited.max_glibc = Some("2.17".into());

    let error = check_device(&spec, &package(limited.clone()), &limited, &tree)
        .err()
        .expect("three problems");
    assert!(error.contains("(3 problems)"), "{error}");
    assert!(error.contains("/usr/bin/a"), "{error}");
    assert!(error.contains("/usr/bin/b"), "{error}");
}
