//! `[cpu]` profiles: the flags reach rustc, next to the project's own.

use super::*;
use gaia_testkit::TempDir;

/// Whether the standard library for `target` is installed, so a library
/// crate can be compiled for it (no linker is needed for an rlib).
fn std_installed(target: &str) -> bool {
    let Ok(output) = Command::new("rustc").args(["--print", "sysroot"]).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Path::new(&sysroot)
        .join("lib/rustlib")
        .join(target)
        .is_dir()
}

/// A library crate that compiles only with dotprod enabled and with the
/// cfg its own `.cargo/config.toml` passes for the target.
fn dotprod_crate(scratch: &TempDir) -> PathBuf {
    let root = scratch.join("cpu-crate");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::create_dir_all(root.join(".cargo")).expect(".cargo");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"kernels\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .expect("manifest");
    fs::write(
        root.join("src/lib.rs"),
        "#[cfg(not(target_feature = \"dotprod\"))]\ncompile_error!(\"built without dotprod\");\n\
         #[cfg(not(from_project))]\ncompile_error!(\"the project's rustflags were dropped\");\n\
         pub fn dot(a: &[i8], b: &[i8]) -> i32 { a.iter().zip(b).map(|(x, y)| *x as i32 * *y as i32).sum() }\n",
    )
    .expect("lib");
    fs::write(
        root.join(".cargo/config.toml"),
        "[target.aarch64-unknown-linux-gnu]\nrustflags = [\"--cfg\", \"from_project\"]\n",
    )
    .expect("config");
    root
}

fn build(root: &Path, cpu: Option<gaia_spec::RustCpuSpec>) -> Result<(), ArtifactProviderError> {
    let mut artifact = rust_artifact("kernels", "kernels", &root.join("out/kernels"));
    artifact.target = Some("aarch64-unknown-linux-gnu".into());
    let contract = nested_contract(&artifact, root);
    let flags = CargoFeatureFlags {
        cpu,
        ..CargoFeatureFlags::default()
    };
    let target = CargoTarget {
        dir: root.join("target"),
        shared: false,
        note: None,
    };
    run_cargo_build(
        &root.display().to_string(),
        &["kernels".to_string()],
        &flags,
        &contract,
        &target,
        None,
        None,
    )
}

#[test]
fn a_cpu_profile_reaches_rustc_with_the_projects_own_flags() {
    if !std_installed("aarch64-unknown-linux-gnu") {
        eprintln!("skipped: the aarch64-unknown-linux-gnu standard library is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-rust-cpu");
    let root = dotprod_crate(&scratch);
    let error = build(&root, None).expect_err("generic armv8-a has no dotprod");
    assert!(error.message.contains("cargo build"), "{}", error.message);

    let cpu = gaia_spec::cpu_profile("cortex-a76")
        .expect("profile")
        .for_rust_target("aarch64-unknown-linux-gnu");
    build(&root, cpu).expect("cortex-a76 has dotprod, and the project's cfg is kept");
}

#[test]
fn cpu_flags_join_the_toolchain_and_inherited_flags() {
    let cpu = gaia_spec::cpu_profile("cortex-a7")
        .expect("profile")
        .for_rust_target("armv7-unknown-linux-gnueabihf");
    let inherited = |name: &str| match name {
        "RUSTFLAGS" => Some("-C debuginfo=0".to_string()),
        "CFLAGS_armv7_unknown_linux_gnueabihf" => Some("-O2".to_string()),
        _ => None,
    };
    let env = toolchain::with_cpu_flags(
        Vec::new(),
        cpu.as_ref(),
        Some("armv7-unknown-linux-gnueabihf"),
        inherited,
    );
    let value = |key: &str| {
        env.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(
        value("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS"),
        Some("-C debuginfo=0 -C target-cpu=cortex-a7 -C target-feature=+neon,+vfp4")
    );
    assert_eq!(
        value("CFLAGS_armv7_unknown_linux_gnueabihf"),
        Some("-O2 -mcpu=cortex-a7 -mfpu=neon-vfpv4")
    );
    assert_eq!(
        value("CXXFLAGS_armv7_unknown_linux_gnueabihf"),
        Some("-mcpu=cortex-a7 -mfpu=neon-vfpv4")
    );

    // A toolchain's flags come first; the CPU's are appended.
    let env = toolchain::with_cpu_flags(
        vec![(
            "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS".into(),
            "-C link-arg=--sysroot=/sdk".into(),
        )],
        cpu.as_ref(),
        Some("armv7-unknown-linux-gnueabihf"),
        |_| None,
    );
    assert_eq!(
        env[0].1,
        "-C link-arg=--sysroot=/sdk -C target-cpu=cortex-a7 -C target-feature=+neon,+vfp4"
    );

    // No profile, or no target: nothing is added.
    assert!(toolchain::with_cpu_flags(Vec::new(), None, Some("armv7"), |_| None).is_empty());
    assert!(toolchain::with_cpu_flags(Vec::new(), cpu.as_ref(), None, |_| None).is_empty());
}

#[test]
fn an_artifacts_own_target_flags_stay_under_the_cpu_flags() {
    // Atlas's board-lite on the K230D: build_env asks for +crt-static, and the
    // CPU profile used to replace it, so the binary linked dynamically.
    let target = "riscv64gc-unknown-linux-musl";
    let cpu = gaia_spec::cpu_profile("xuantie-c908")
        .expect("profile")
        .for_rust_target(target);
    let key = "CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_RUSTFLAGS";
    let build_env = vec![(key.to_string(), "-C target-feature=+crt-static".to_string())];
    let inherited = |name: &str| (name == key).then(|| "-C debuginfo=0".to_string());
    let env = toolchain::with_build_env_and_cpu_flags(
        &build_env,
        Vec::new(),
        cpu.as_ref(),
        Some(target),
        inherited,
    );
    let flags = &env
        .iter()
        .find(|(name, _)| name == key)
        .expect("target flags")
        .1;
    // The artifact's value comes first and wins over the process's.
    assert!(
        flags.starts_with("-C target-feature=+crt-static -C target-feature=+v"),
        "{flags}"
    );
    assert!(!flags.contains("debuginfo"), "{flags}");
}
