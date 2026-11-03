//! The environment of a cross build: a rust artifact built with a
//! `[[toolchains]]` SDK gets the SDK's linker and C compiler (its compiler
//! prefix in `bin/`), the sysroot for every compile and link, and the
//! pkg-config of the sysroot. Cargo and the `cc` crate read these names; the
//! flags the environment already gives (`RUSTFLAGS`, `CFLAGS_<target>`) are kept
//! in front of Gaia's own.

use gaia_spec::RustCpuSpec;
use std::path::{Path, PathBuf};
use std::process::Command;

use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    ArtifactToolchainContract,
};

/// The environment a cargo invocation of `contract` adds for its toolchain:
/// nothing for a host build.
pub(crate) fn cargo_env(
    contract: &ArtifactExecutionContract,
    source_dir: &Path,
) -> Result<Vec<(String, String)>, ArtifactProviderError> {
    let Some(toolchain) = &contract.toolchain else {
        return Ok(Vec::new());
    };
    let target = contract.artifact_target.as_deref().ok_or_else(|| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "toolchain '{}' builds rust for no target: set the artifact's target or the toolchain's rust_target",
                toolchain.id
            ),
        )
    })?;
    let gcc_ld = if toolchain.lld {
        Some(rust_gcc_ld_dir(source_dir).map_err(|message| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!("toolchain '{}' links with lld: {message}", toolchain.id),
            )
        })?)
    } else {
        None
    };
    Ok(env_for(toolchain, target, gcc_ld.as_deref(), |name| {
        std::env::var(name).ok()
    }))
}

/// The directory of Rust's `gcc-ld` wrappers (`ld.lld` running `rust-lld`)
/// of the rustc that builds in `source_dir`, which may pin its own toolchain.
fn rust_gcc_ld_dir(source_dir: &Path) -> Result<PathBuf, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let run = |args: &[&str]| -> Result<String, String> {
        let output = Command::new(&rustc)
            .args(args)
            .current_dir(source_dir)
            .output()
            .map_err(|error| format!("cannot run '{rustc}': {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "'{rustc} {}' failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let sysroot = run(&["--print", "sysroot"])?;
    let version = run(&["-vV"])?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or_else(|| format!("'{rustc} -vV' names no host"))?;
    let dir = Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(host.trim())
        .join("bin/gcc-ld");
    if !dir.join("ld.lld").is_file() {
        return Err(format!(
            "'{}' has no ld.lld: the rust toolchain needs its rust-lld (rustup's default profile has it)",
            dir.display()
        ));
    }
    Ok(dir)
}

/// The environment for `target`, with `inherited` the values the calling
/// environment already has (looked up by name) and `gcc_ld` Rust's `gcc-ld`
/// directory for a toolchain that links with lld.
pub(crate) fn env_for(
    toolchain: &ArtifactToolchainContract,
    target: &str,
    gcc_ld: Option<&Path>,
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    // Cargo's per-target variables spell the triple in upper case, the `cc`
    // crate's in lower case; both use underscores for the dashes.
    let upper = target.replace('-', "_").to_ascii_uppercase();
    let lower = target.replace('-', "_");
    let sysroot = toolchain.sysroot.display().to_string();
    let sysroot_flag = format!("--sysroot={sysroot}");
    let gcc = toolchain.tool("gcc").display().to_string();
    let gxx = toolchain.tool("g++").display().to_string();
    let ar = toolchain.tool("ar").display().to_string();

    let rustflags_base = inherited(&format!("CARGO_TARGET_{upper}_RUSTFLAGS"))
        .or_else(|| inherited("RUSTFLAGS"))
        .unwrap_or_default();
    // A declared page size links the binaries for that page size, so a device
    // whose kernel pages are larger than the SDK's default can load them.
    let sysroot_link = format!("link-arg={sysroot_flag}");
    let page_link = toolchain
        .page_size
        .map(|page| format!("link-arg=-Wl,-z,max-page-size={page}"));
    let mut flags = vec![rustflags_base.as_str(), "-C", sysroot_link.as_str()];
    if let Some(page_link) = &page_link {
        flags.push("-C");
        flags.push(page_link.as_str());
    }
    // The SDK's compiler still drives the link (its crt files and libraries),
    // with Rust's LLD in place of its `ld`.
    let lld_dir = gcc_ld.map(|dir| format!("link-arg=-B{}", dir.display()));
    if let Some(lld_dir) = &lld_dir {
        flags.extend(["-C", lld_dir.as_str(), "-C", "link-arg=-fuse-ld=lld"]);
    }
    let rustflags = join_flags(&flags);
    let cflags = join_flags(&[
        inherited(&format!("CFLAGS_{lower}"))
            .unwrap_or_default()
            .as_str(),
        &sysroot_flag,
    ]);
    let cxxflags = join_flags(&[
        inherited(&format!("CXXFLAGS_{lower}"))
            .unwrap_or_default()
            .as_str(),
        &sysroot_flag,
    ]);
    vec![
        (format!("CARGO_TARGET_{upper}_LINKER"), gcc.clone()),
        (format!("CARGO_TARGET_{upper}_RUSTFLAGS"), rustflags),
        (format!("CC_{lower}"), gcc),
        (format!("CXX_{lower}"), gxx),
        (format!("AR_{lower}"), ar),
        (format!("CFLAGS_{lower}"), cflags),
        (format!("CXXFLAGS_{lower}"), cxxflags),
        ("PKG_CONFIG_SYSROOT_DIR".into(), sysroot.clone()),
        (
            "PKG_CONFIG_PATH".into(),
            format!("{sysroot}/usr/lib/pkgconfig:{sysroot}/usr/share/pkgconfig"),
        ),
        ("PKG_CONFIG_ALLOW_CROSS".into(), "1".into()),
    ]
}

/// `env` with the board's CPU flags (`[cpu]`) added for `target`: to the
/// rustc flags cargo reads for the target, and to the C and C++ flags the `cc`
/// crate reads. A value `env` does not set starts from the calling
/// environment's, as the toolchain's do; `RUSTFLAGS` is folded into the
/// target's flags, since cargo would otherwise use it instead of them.
pub(crate) fn with_cpu_flags(
    mut env: Vec<(String, String)>,
    cpu: Option<&RustCpuSpec>,
    target: Option<&str>,
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let (Some(cpu), Some(target)) = (cpu, target) else {
        return env;
    };
    let upper = target.replace('-', "_").to_ascii_uppercase();
    let lower = target.replace('-', "_");
    let rustflags = cpu.rustflags.join(" ");
    let c_flags = cpu.c_flags.join(" ");
    for (name, extra, fallback) in [
        (
            format!("CARGO_TARGET_{upper}_RUSTFLAGS"),
            &rustflags,
            Some("RUSTFLAGS"),
        ),
        (format!("CFLAGS_{lower}"), &c_flags, None),
        (format!("CXXFLAGS_{lower}"), &c_flags, None),
    ] {
        match env.iter_mut().find(|(key, _)| *key == name) {
            Some((_, value)) => *value = join_flags(&[value.as_str(), extra]),
            None => {
                let base = inherited(&name)
                    .or_else(|| fallback.and_then(&inherited))
                    .unwrap_or_default();
                env.push((name, join_flags(&[base.as_str(), extra])));
            }
        }
    }
    env
}

/// [`with_cpu_flags`] for a cargo run that also has the artifact's
/// `build_env`: its values are the base, ahead of the process environment, so
/// a package's own target flags (`+crt-static`, say) stay under the CPU's.
pub(crate) fn with_build_env_and_cpu_flags(
    build_env: &[(String, String)],
    toolchain_env: Vec<(String, String)>,
    cpu: Option<&RustCpuSpec>,
    target: Option<&str>,
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    with_cpu_flags(toolchain_env, cpu, target, |name| {
        build_env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| inherited(name))
    })
}

/// Whether `env` sets a target's `CARGO_TARGET_<T>_RUSTFLAGS`.
pub(crate) fn sets_target_rustflags(env: &[(String, String)]) -> bool {
    env.iter()
        .any(|(key, _)| key.starts_with("CARGO_TARGET_") && key.ends_with("_RUSTFLAGS"))
}

/// The non-empty flags joined by single spaces.
fn join_flags(flags: &[&str]) -> String {
    flags
        .iter()
        .map(|flag| flag.trim())
        .filter(|flag| !flag.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolchain() -> ArtifactToolchainContract {
        ArtifactToolchainContract {
            id: "systemcore".into(),
            root: PathBuf::from("/sdk"),
            sysroot: PathBuf::from("/sdk/aarch64-buildroot-linux-gnu/sysroot"),
            prefix: "aarch64-buildroot-linux-gnu-".into(),
            triple: "aarch64-buildroot-linux-gnu".into(),
            rust_target: Some("aarch64-unknown-linux-gnu".into()),
            page_size: None,
            lld: false,
            relocate: false,
        }
    }

    #[test]
    fn a_declared_page_size_links_with_max_page_size_after_the_sysroot_flag() {
        let mut sdk = toolchain();
        sdk.page_size = Some(16384);
        let env = env_for(&sdk, "aarch64-unknown-linux-gnu", None, |name| match name {
            "RUSTFLAGS" => Some("-C target-cpu=cortex-a53".into()),
            _ => None,
        });
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"),
            "-C target-cpu=cortex-a53 -C link-arg=--sysroot=/sdk/aarch64-buildroot-linux-gnu/sysroot -C link-arg=-Wl,-z,max-page-size=16384"
        );
    }

    #[test]
    fn without_a_page_size_the_link_flags_are_the_sysroot_flag_alone() {
        let env = env_for(&toolchain(), "aarch64-unknown-linux-gnu", None, |_| None);
        assert!(
            !value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS")
                .contains("max-page-size")
        );
    }

    #[test]
    fn an_lld_toolchain_links_through_rusts_lld_after_the_sysroot_flag() {
        let mut sdk = toolchain();
        sdk.lld = true;
        let env = env_for(
            &sdk,
            "aarch64-unknown-linux-gnu",
            Some(Path::new(
                "/rust/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld",
            )),
            |_| None,
        );
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"),
            "-C link-arg=--sysroot=/sdk/aarch64-buildroot-linux-gnu/sysroot -C link-arg=-B/rust/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld -C link-arg=-fuse-ld=lld"
        );
        // The SDK's compiler stays the linker driver.
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER"),
            "/sdk/bin/aarch64-buildroot-linux-gnu-gcc"
        );
    }

    #[test]
    fn the_gcc_ld_dir_is_the_building_rustcs() {
        let dir = rust_gcc_ld_dir(Path::new(".")).expect("gcc-ld");
        assert!(dir.join("ld.lld").is_file(), "{}", dir.display());
    }

    fn value<'a>(env: &'a [(String, String)], name: &str) -> &'a str {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("{name} is not set"))
    }

    #[test]
    fn the_linker_and_compilers_come_from_the_sdk_prefix() {
        let env = env_for(&toolchain(), "aarch64-unknown-linux-gnu", None, |_| None);
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER"),
            "/sdk/bin/aarch64-buildroot-linux-gnu-gcc"
        );
        assert_eq!(
            value(&env, "CC_aarch64_unknown_linux_gnu"),
            "/sdk/bin/aarch64-buildroot-linux-gnu-gcc"
        );
        assert_eq!(
            value(&env, "CXX_aarch64_unknown_linux_gnu"),
            "/sdk/bin/aarch64-buildroot-linux-gnu-g++"
        );
        assert_eq!(
            value(&env, "AR_aarch64_unknown_linux_gnu"),
            "/sdk/bin/aarch64-buildroot-linux-gnu-ar"
        );
    }

    #[test]
    fn the_sysroot_reaches_every_compile_and_link() {
        let env = env_for(&toolchain(), "aarch64-unknown-linux-gnu", None, |_| None);
        let sysroot = "--sysroot=/sdk/aarch64-buildroot-linux-gnu/sysroot";
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"),
            format!("-C link-arg={sysroot}")
        );
        assert_eq!(value(&env, "CFLAGS_aarch64_unknown_linux_gnu"), sysroot);
        assert_eq!(value(&env, "CXXFLAGS_aarch64_unknown_linux_gnu"), sysroot);
        assert_eq!(
            value(&env, "PKG_CONFIG_SYSROOT_DIR"),
            "/sdk/aarch64-buildroot-linux-gnu/sysroot"
        );
        assert_eq!(
            value(&env, "PKG_CONFIG_PATH"),
            "/sdk/aarch64-buildroot-linux-gnu/sysroot/usr/lib/pkgconfig:/sdk/aarch64-buildroot-linux-gnu/sysroot/usr/share/pkgconfig"
        );
        assert_eq!(value(&env, "PKG_CONFIG_ALLOW_CROSS"), "1");
    }

    #[test]
    fn inherited_rustflags_are_kept_in_front_of_the_sysroot_flag() {
        let env = env_for(
            &toolchain(),
            "aarch64-unknown-linux-gnu",
            None,
            |name| match name {
                "RUSTFLAGS" => Some("-C target-cpu=cortex-a53".into()),
                _ => None,
            },
        );
        assert_eq!(
            value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"),
            "-C target-cpu=cortex-a53 -C link-arg=--sysroot=/sdk/aarch64-buildroot-linux-gnu/sysroot"
        );
    }

    #[test]
    fn a_target_specific_rustflags_variable_wins_over_the_generic_one() {
        let env = env_for(
            &toolchain(),
            "aarch64-unknown-linux-gnu",
            None,
            |name| match name {
                "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS" => Some("-C opt-level=2".into()),
                "RUSTFLAGS" => Some("-C debuginfo=0".into()),
                _ => None,
            },
        );
        let flags = value(&env, "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS");
        assert!(flags.starts_with("-C opt-level=2 "), "{flags}");
        assert!(!flags.contains("debuginfo"), "{flags}");
    }
}
