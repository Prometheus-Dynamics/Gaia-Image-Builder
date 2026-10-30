//! The `[[toolchains]]` SDK a rust artifact cross-compiles with, as the artifact
//! providers see it, and the one-time setup the SDK needs before its first use.

use std::path::{Path, PathBuf};
use std::process::Command;

use gaia_spec::ResolvedToolchain;

/// The file a relocated SDK root gets, so `relocate-sdk.sh` runs once per
/// extracted location.
pub(crate) const RELOCATED_MARKER: &str = ".gaia-relocated";

/// A toolchain resolved for one artifact build: its paths on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactToolchainContract {
    pub id: String,
    /// The SDK root (`bin/`, `relocate-sdk.sh` and the sysroot live under it).
    pub root: PathBuf,
    pub sysroot: PathBuf,
    /// The compiler prefix in `<root>/bin`.
    pub prefix: String,
    pub triple: String,
    pub rust_target: Option<String>,
    /// The device page size, when declared: Rust artifacts link with it.
    pub page_size: Option<u64>,
    pub relocate: bool,
}

impl ArtifactToolchainContract {
    pub fn from_resolved(resolved: &ResolvedToolchain, relocate: bool) -> Self {
        Self {
            id: resolved.id.clone(),
            root: resolved.root.clone(),
            sysroot: resolved.sysroot.clone(),
            prefix: resolved.prefix.clone(),
            triple: resolved.triple.clone(),
            rust_target: resolved.rust_target.clone(),
            page_size: resolved.page_size,
            relocate,
        }
    }

    /// The compiler or tool `name` in the SDK's `bin` dir, with the prefix.
    pub fn tool(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(format!("{}{name}", self.prefix))
    }
}

/// Runs `<root>/relocate-sdk.sh` once for an SDK root, when the toolchain asks
/// for it: the marker file records the run, so later builds skip it. An SDK
/// without the script is used as it is, and the returned note says so. `None`
/// when there is nothing to say.
pub fn ensure_relocated(toolchain: &ArtifactToolchainContract) -> Result<Option<String>, String> {
    if !toolchain.relocate {
        return Ok(None);
    }
    let marker = toolchain.root.join(RELOCATED_MARKER);
    if marker.is_file() {
        return Ok(None);
    }
    let script = toolchain.root.join("relocate-sdk.sh");
    if !script.is_file() {
        return Ok(Some(format!(
            "toolchain '{}': '{}' is absent, so the SDK is used as it is",
            toolchain.id,
            script.display()
        )));
    }
    let output = Command::new("sh")
        .arg(&script)
        .current_dir(&toolchain.root)
        .output()
        .map_err(|error| {
            format!(
                "toolchain '{}': failed to run '{}': {error}",
                toolchain.id,
                script.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "toolchain '{}': '{}' failed: {}",
            toolchain.id,
            script.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    write_marker(&marker).map_err(|error| {
        format!(
            "toolchain '{}': failed to record the relocation in '{}': {error}",
            toolchain.id,
            marker.display()
        )
    })?;
    Ok(Some(format!(
        "toolchain '{}': ran '{}' once for this SDK root",
        toolchain.id,
        script.display()
    )))
}

fn write_marker(marker: &Path) -> std::io::Result<()> {
    std::fs::write(marker, b"relocated by gaia\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::fs;

    fn toolchain_at(root: &Path, relocate: bool) -> ArtifactToolchainContract {
        ArtifactToolchainContract {
            id: "sdk".into(),
            root: root.to_path_buf(),
            sysroot: root.join("sysroot"),
            prefix: "aarch64-linux-".into(),
            triple: "aarch64-buildroot-linux-gnu".into(),
            rust_target: None,
            page_size: None,
            relocate,
        }
    }

    #[test]
    fn relocate_runs_the_script_once_and_the_marker_skips_it_after() {
        let scratch = TempDir::new("gaia-sdk-relocate");
        let root = scratch.join("sdk");
        fs::create_dir_all(&root).expect("sdk root");
        // The script records each run, so a second run would show twice.
        fs::write(root.join("relocate-sdk.sh"), "echo ran >> runs.txt\n").expect("script");
        let toolchain = toolchain_at(&root, true);

        let first = ensure_relocated(&toolchain).expect("first run");
        assert!(first.is_some_and(|note| note.contains("ran")));
        assert!(root.join(RELOCATED_MARKER).is_file());
        assert_eq!(fs::read_to_string(root.join("runs.txt")).unwrap(), "ran\n");

        assert_eq!(ensure_relocated(&toolchain).expect("second run"), None);
        assert_eq!(fs::read_to_string(root.join("runs.txt")).unwrap(), "ran\n");
    }

    #[test]
    fn an_sdk_without_the_script_is_used_as_it_is_with_a_note() {
        let scratch = TempDir::new("gaia-sdk-no-script");
        let root = scratch.join("sdk");
        fs::create_dir_all(&root).expect("sdk root");
        let note = ensure_relocated(&toolchain_at(&root, true))
            .expect("not an error")
            .expect("a note");
        assert!(note.contains("is absent"), "{note}");
        assert!(!root.join(RELOCATED_MARKER).exists());
    }

    #[test]
    fn a_toolchain_that_does_not_relocate_is_left_alone() {
        let scratch = TempDir::new("gaia-sdk-no-relocate");
        let root = scratch.join("sdk");
        fs::create_dir_all(&root).expect("sdk root");
        fs::write(root.join("relocate-sdk.sh"), "exit 1\n").expect("script");
        assert_eq!(
            ensure_relocated(&toolchain_at(&root, false)).expect("untouched"),
            None
        );
    }

    #[test]
    fn a_failing_relocation_is_an_error_and_leaves_no_marker() {
        let scratch = TempDir::new("gaia-sdk-fail");
        let root = scratch.join("sdk");
        fs::create_dir_all(&root).expect("sdk root");
        fs::write(root.join("relocate-sdk.sh"), "echo broken >&2\nexit 3\n").expect("script");
        let error = ensure_relocated(&toolchain_at(&root, true)).expect_err("fails");
        assert!(error.contains("broken"), "{error}");
        assert!(!root.join(RELOCATED_MARKER).exists());
    }
}
