//! A real cross build against a Buildroot SDK, as a kind-none build: a rust
//! artifact is built for the SDK's aarch64 target, put in an assembly tree and
//! packed as an ipk whose device checks read the SDK's glibc sysroot.
//!
//! Ignored by default, because it runs cargo for a cross target and needs an
//! SDK on this machine. It skips (passing) when the SDK directory is missing.
//! Run: `cargo test -p gaia-app --test toolchain_device_real -- --ignored --nocapture`.
//! The SDK is only read, never written.

pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::write_temp_build;

/// The aarch64 glibc Buildroot SDK's `host` directory (read-only here).
const SDK_HOST: &str = "/dev/shm/gaia-sozo/ca5bb1fdc719/buildroot-output/host";

/// The build config: the hello crate, the SDK as a `path` toolchain, the
/// artifact in an assembly tree, and the ipk with its device checks.
/// `extra_files` adds assembly files; `device_lines` replaces the device table.
fn build_config(
    root: &Path,
    page_size: Option<u64>,
    extra_files: &str,
    device_lines: &str,
) -> String {
    let page_line = page_size
        .map(|page| format!("page_size = {page}\n"))
        .unwrap_or_default();
    format!(
        r#"
build_name = "systemcore-hello"
version = "1.0.0"

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[providers.rust]
allow_nested_build = true

[[sources]]
id = "hello"
kind = "path"
path = "hello"
refresh = "never"
pin = "locked"

[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
path = "{sdk}"
prefix = "aarch64-linux-"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"
rust_target = "aarch64-unknown-linux-gnu"
{page_line}
[[artifacts]]
id = "hello"
kind = "rust"
package = "hello"
source = "hello"
toolchain = "systemcore"
output_path = "${{workspace.out_dir}}/artifacts/hello"

[image]
kind = "none"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"
out_dir = "${{workspace.out_dir}}"

[[image.assembly.trees]]
id = "package-root"
path = "$assembly.work/package-root"

[[image.assembly.files]]
tree = "package-root"
src = "${{workspace.out_dir}}/artifacts/hello"
dest = "usr/bin/hello"
mode = "0755"
{extra_files}

[[image.assembly.packages]]
id = "hello"
tree = "package-root"
dest = "$assembly.out/"
allowed_prefixes = ["/usr/bin"]

[image.assembly.packages.control]
Package = "systemcore-hello"
Version = "${{build.version}}"
Architecture = "aarch64"
Maintainer = "Test <test@example.invalid>"
Description = "A hello built for SystemCore."

{device_lines}
"#,
        root = root.display(),
        sdk = SDK_HOST,
    )
}

/// The hello crate: a binary with no dependencies.
fn write_hello(root: &Path) {
    let crate_dir = root.join("hello");
    fs::create_dir_all(crate_dir.join("src")).expect("hello dir");
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    fs::write(
        crate_dir.join("src/main.rs"),
        "fn main() {\n    println!(\"hello from SystemCore\");\n}\n",
    )
    .expect("main.rs");
}

/// Whether the SDK is on this machine; a missing one skips the test.
fn sdk_present() -> bool {
    let present = Path::new(SDK_HOST)
        .join("bin")
        .join("aarch64-linux-gcc")
        .is_file();
    if !present {
        eprintln!("skipped: no aarch64 Buildroot SDK at {SDK_HOST}");
    }
    present
}

/// Everything a run reports as a failure, as one string to search.
fn failures_of(outcome: &CommandOutcome) -> String {
    match outcome {
        CommandOutcome::Ran {
            report,
            execution_errors,
            validation,
            ..
        } => format!(
            "{execution_errors:?}\n{:?}\n{:?}",
            validation.errors, report.summary.error_count
        ),
        other => format!("{other:?}"),
    }
}

fn run(build: String) -> CommandOutcome {
    let outcome = run_with_args(AppArgs {
        command: AppCommand::Run,
        build: build.clone(),
        ..AppArgs::default()
    });
    if let CommandOutcome::Failed { message } = &outcome {
        // The refusal names a count only: show what validation found.
        eprintln!("run refused: {message}");
        let checked = run_with_args(AppArgs {
            command: AppCommand::Validate,
            build,
            ..AppArgs::default()
        });
        if let CommandOutcome::Validated { validation, .. } = &checked {
            eprintln!("validation errors: {:?}", validation.errors);
        } else {
            eprintln!("validate: {checked:?}");
        }
    }
    outcome
}

/// The ELF's description from `file`, which reads the bytes and runs nothing.
fn file_says(path: &Path) -> String {
    let output = Command::new("file")
        .arg(path)
        .output()
        .expect("file runs on this machine");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The package's device table: the SDK's sysroot, with an explicit
/// `min_page_size` only when one is given (otherwise the toolchain's page size).
fn device_table(max_glibc: Option<&str>, min_page_size: Option<u64>) -> String {
    let max = max_glibc
        .map(|value| format!("max_glibc = \"{value}\"\n"))
        .unwrap_or_default();
    let min = min_page_size
        .map(|value| format!("min_page_size = {value}\n"))
        .unwrap_or_default();
    format!(
        "[image.assembly.packages.device]\ntoolchain = \"systemcore\"\n{max}{min}check_needed = true\n"
    )
}

#[test]
#[ignore = "needs the aarch64 Buildroot SDK at /dev/shm; run with --ignored"]
fn a_cross_built_hello_packs_into_an_ipk_that_passes_the_device_checks() {
    if !sdk_present() {
        return;
    }
    let scratch = TempDir::new("gaia-systemcore-good");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_hello(&root);
    let build = write_temp_build(
        &scratch,
        &build_config(&root, Some(16384), "", &device_table(None, None)),
    );

    let outcome = run(build);
    let failures = failures_of(&outcome);
    match &outcome {
        CommandOutcome::Ran {
            report,
            execution_errors,
            validation,
            ..
        } => {
            assert!(validation.errors.is_empty(), "{:?}", validation.errors);
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            assert_eq!(report.summary.error_count, 0, "{failures}");
            let primary = report
                .summary
                .primary_image_output
                .clone()
                .expect("the package is the primary output");
            let primary = PathBuf::from(primary);
            assert!(
                primary.extension().is_some_and(|ext| ext == "ipk") && primary.is_file(),
                "{}",
                primary.display()
            );
            let binary = root.join("out/artifacts/hello");
            let description = file_says(&binary);
            println!("hello: {description}");
            assert!(description.contains("ARM aarch64"), "{description}");
        }
        other => panic!("expected a run outcome, got {other:?}"),
    }
}

#[test]
#[ignore = "needs the aarch64 Buildroot SDK at /dev/shm; run with --ignored"]
fn a_host_x86_64_binary_in_the_package_fails_the_device_checks() {
    if !sdk_present() {
        return;
    }
    let scratch = TempDir::new("gaia-systemcore-x86");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_hello(&root);
    let extra = "\n[[image.assembly.files]]\ntree = \"package-root\"\nsrc = \"/bin/true\"\ndest = \"usr/bin/host-true\"\nmode = \"0755\"\n";
    let build = write_temp_build(
        &scratch,
        &build_config(&root, Some(16384), extra, &device_table(None, None)),
    );

    let outcome = run(build);
    let failures = failures_of(&outcome);
    println!("{failures}");
    assert!(failures.contains("host-true"), "{failures}");
    assert!(failures.contains("x86-64"), "{failures}");
    assert!(failures.contains("device checks"), "{failures}");
}

#[test]
#[ignore = "needs the aarch64 Buildroot SDK at /dev/shm; run with --ignored"]
fn a_max_glibc_below_what_the_binary_needs_fails_the_device_checks() {
    if !sdk_present() {
        return;
    }
    let scratch = TempDir::new("gaia-systemcore-glibc");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_hello(&root);
    let build = write_temp_build(
        &scratch,
        &build_config(&root, Some(16384), "", &device_table(Some("2.17"), None)),
    );

    let outcome = run(build);
    let failures = failures_of(&outcome);
    println!("{failures}");
    assert!(
        failures.contains("usr/bin/hello") || failures.contains("/usr/bin/hello"),
        "{failures}"
    );
    assert!(failures.contains("GLIBC_2."), "{failures}");
    assert!(failures.contains("max_glibc 2.17"), "{failures}");
}

#[test]
#[ignore = "needs the aarch64 Buildroot SDK at /dev/shm; run with --ignored"]
fn the_sdks_default_4k_segments_fail_a_16k_page_device() {
    if !sdk_present() {
        return;
    }
    // The toolchain names no page size, so the SDK's linker default (4 KiB
    // segments) stands, and the device asks for 16 KiB explicitly.
    let scratch = TempDir::new("gaia-systemcore-4k");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_hello(&root);
    let build = write_temp_build(
        &scratch,
        &build_config(&root, None, "", &device_table(None, Some(16384))),
    );

    let outcome = run(build);
    let failures = failures_of(&outcome);
    println!("{failures}");
    assert!(failures.contains("p_align 0x1000"), "{failures}");
    assert!(failures.contains("min_page_size 0x4000"), "{failures}");
}
