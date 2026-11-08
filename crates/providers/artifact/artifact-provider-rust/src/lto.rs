//! `lto = "linker"`: link-time optimization by the linker instead of rustc.
//!
//! Rustc refuses its own LTO with `-C prefer-dynamic`, so a shared build
//! loses inlining across crates. With `-C linker-plugin-lto` rustc writes
//! LLVM bitcode instead of machine code and the linker optimizes each link
//! (a library and the crates linked into it, a program and its own) as one
//! module. Only an LLVM linker reads that bitcode, so the link goes through
//! rust-lld, the one shipped with the rustc that wrote the bitcode, from the
//! rustc in the artifact's execution backend: the `gcc-ld` directory on the
//! linker driver's program path (`-B`) and `-fuse-ld=lld`. The cargo
//! profile's own `lto` is turned off for the build.

use crate::toolchain::with_target_flags;
use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    command_for_execution, command_output_with_timeout,
};
use gaia_spec::{BuildModeSpec, RustLtoSpec};
use std::process::Command;
use std::time::Duration;

/// `env` with the rustc and cargo settings of `lto` for a build of `target`
/// (`None`: the host) in `build_mode`, rust-lld's wrappers in `gcc_ld`. A
/// value `env` does not set starts from `inherited`'s.
pub(crate) fn with_lto_flags(
    mut env: Vec<(String, String)>,
    lto: RustLtoSpec,
    target: Option<&str>,
    build_mode: Option<&BuildModeSpec>,
    gcc_ld: &str,
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    if lto.is_profile() {
        return env;
    }
    let flags = format!("-C linker-plugin-lto -C link-arg=-B{gcc_ld} -C link-arg=-fuse-ld=lld");
    env = with_target_flags(env, &flags, target, inherited);
    env.push((
        format!("CARGO_PROFILE_{}_LTO", profile_variable(build_mode)),
        "off".into(),
    ));
    env
}

/// The cargo profile's name as its `CARGO_PROFILE_<NAME>_*` variables spell
/// it; cargo's default build is `dev`.
fn profile_variable(build_mode: Option<&BuildModeSpec>) -> String {
    match build_mode {
        Some(BuildModeSpec::Release) => "RELEASE".into(),
        Some(BuildModeSpec::Debug) | None => "DEV".into(),
        Some(BuildModeSpec::Custom(profile)) => profile.replace('-', "_").to_ascii_uppercase(),
    }
}

/// Prints the `gcc-ld` directory of the rustc that builds in the current
/// directory (its `rust-toolchain.toml` honoured), and fails without its
/// `ld.lld`.
const GCC_LD_SCRIPT: &str = r#"set -e
sysroot=$(rustc --print sysroot)
host=$(rustc -vV | sed -n 's/^host: //p')
dir=$sysroot/lib/rustlib/$host/bin/gcc-ld
if [ ! -x "$dir/ld.lld" ]; then
    echo "'$dir' has no ld.lld: the rust toolchain needs its rust-lld (rustup's default and minimal profiles have it)" >&2
    exit 1
fi
printf '%s' "$dir""#;

/// Rust-lld's `gcc-ld` directory as the build sees it: on the host, or in
/// the artifact's container.
pub(crate) fn gcc_ld_dir(
    contract: &ArtifactExecutionContract,
    source_dir: &str,
) -> Result<String, ArtifactProviderError> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(GCC_LD_SCRIPT).current_dir(source_dir);
    let mut command = command_for_execution(&command, contract)?;
    let output = command_output_with_timeout(
        &mut command,
        Duration::from_secs(300),
        "rustc's rust-lld for lto = \"linker\"",
    )?;
    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || dir.is_empty() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "lto = \"linker\" links with rust-lld: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(String, String)], name: &str) -> &'a str {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("{name} is not set"))
    }

    #[test]
    fn linker_lto_adds_bitcode_rust_lld_and_turns_the_profiles_lto_off() {
        let target = "aarch64-unknown-linux-gnu";
        let name = "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS";
        let shared = vec![(name.to_string(), "-C prefer-dynamic".to_string())];
        let env = with_lto_flags(
            shared,
            RustLtoSpec::Linker,
            Some(target),
            Some(&BuildModeSpec::Custom("dev-image".into())),
            "/rust/gcc-ld",
            |_| None,
        );
        assert_eq!(
            value(&env, name),
            "-C prefer-dynamic -C linker-plugin-lto -C link-arg=-B/rust/gcc-ld -C link-arg=-fuse-ld=lld"
        );
        assert_eq!(value(&env, "CARGO_PROFILE_DEV_IMAGE_LTO"), "off");

        let host = with_lto_flags(
            Vec::new(),
            RustLtoSpec::Linker,
            None,
            Some(&BuildModeSpec::Release),
            "/rust/gcc-ld",
            |_| Some("-C target-cpu=native".into()),
        );
        assert!(value(&host, "RUSTFLAGS").starts_with("-C target-cpu=native -C linker-plugin-lto"));
        assert_eq!(value(&host, "CARGO_PROFILE_RELEASE_LTO"), "off");
    }

    #[test]
    fn profile_lto_changes_nothing() {
        assert!(
            with_lto_flags(
                Vec::new(),
                RustLtoSpec::Profile,
                Some("aarch64-unknown-linux-gnu"),
                None,
                "/rust/gcc-ld",
                |_| None,
            )
            .is_empty()
        );
    }
}
