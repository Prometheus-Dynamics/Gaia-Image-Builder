//! Changes Gaia makes to the Buildroot source it builds with. Each one is
//! applied only to the exact upstream text it replaces, and also exists as
//! an upstreamable patch under `contrib/buildroot/`.
//!
//! With per-package directories, Buildroot assembles the final `target/`
//! and `host/` by copying every package's tree (one `rsync`, which reads and
//! writes every file). On a filesystem with reflinks (btrfs, XFS) a
//! copy-on-write clone (`cp --reflink`) makes the same files without
//! copying their data, and keeps them independent: later in-place edits
//! (stripping, `patchelf`, post-build scripts) do not reach the per-package
//! trees. The last package's tree still wins where two hold the same path.
//! Without reflinks, `cp` would copy everything on every `make` where
//! `rsync` copies only what changed, so the change is applied only when the
//! output directory supports reflinks.

use super::*;

/// `per-package-rsync`'s copy mode in `package/pkg-utils.mk`, as upstream
/// has it since 2024.08.
const PPD_COPY_UPSTREAM: &str = "\t\tprintf \"%s/$(2)/\\n\" $(1) | tac \\\n\t\t\t| rsync -a --hard-links --files-from=- --no-R -r $(PER_PACKAGE_DIR) $(3))";

/// The same, cloning each package's tree in order (the last one wins, as
/// the reversed list makes it win with `rsync`).
const PPD_COPY_REFLINK: &str = "\t\t$(foreach pkg,$(1),\\\n\t\t\tcp -a --reflink=auto --remove-destination $(PER_PACKAGE_DIR)/$(pkg)/$(2)/. $(3)/$(sep)))";

/// Uses reflink clones for the per-package finalize step when the output
/// directory supports them. Returns a message saying what was done.
pub(crate) fn apply_reflink_finalize(
    buildroot_dir: &Path,
    output_dir: &Path,
) -> Result<Option<String>, ImageProviderError> {
    let makefile = buildroot_dir.join("package/pkg-utils.mk");
    let Ok(contents) = fs::read_to_string(&makefile) else {
        return Ok(None);
    };
    if contents.contains(PPD_COPY_REFLINK) {
        return Ok(Some(
            "buildroot per-package finalize uses reflink clones".to_string(),
        ));
    }
    if !contents.contains(PPD_COPY_UPSTREAM) {
        return Ok(Some(
            "buildroot per-package finalize left as is: package/pkg-utils.mk differs from \
             the upstream text Gaia patches"
                .to_string(),
        ));
    }
    if !supports_reflinks(output_dir) {
        return Ok(None);
    }
    fs::write(
        &makefile,
        contents.replace(PPD_COPY_UPSTREAM, PPD_COPY_REFLINK),
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to patch '{}': {error}",
            makefile.display()
        ))
    })?;
    Ok(Some(
        "buildroot per-package finalize uses reflink clones".to_string(),
    ))
}

/// Whether `dir`'s filesystem can clone files (`cp --reflink=always`).
fn supports_reflinks(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".gaia-reflink-probe-{}", std::process::id()));
    let clone = dir.join(format!(".gaia-reflink-probe-{}.clone", std::process::id()));
    let supported = fs::write(&probe, b"gaia").is_ok()
        && Command::new("cp")
            .arg("--reflink=always")
            .arg(&probe)
            .arg(&clone)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    let _ = fs::remove_file(&probe);
    let _ = fs::remove_file(&clone);
    supported
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gaia-buildroot-patch-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn only_the_upstream_text_is_patched_and_only_with_reflinks() {
        let buildroot = temp("source");
        let output = temp("output");
        fs::create_dir_all(buildroot.join("package")).expect("package dir");
        let upstream = format!("define per-package-rsync\n{PPD_COPY_UPSTREAM}\nendef\n");
        fs::write(buildroot.join("package/pkg-utils.mk"), &upstream).expect("makefile");

        let message = apply_reflink_finalize(&buildroot, &output).expect("patch");
        let patched = fs::read_to_string(buildroot.join("package/pkg-utils.mk")).expect("read");
        if supports_reflinks(&output) {
            assert!(patched.contains(PPD_COPY_REFLINK));
            assert!(message.is_some());
            // Applying it again changes nothing.
            apply_reflink_finalize(&buildroot, &output).expect("again");
            assert_eq!(
                fs::read_to_string(buildroot.join("package/pkg-utils.mk")).expect("read"),
                patched
            );
        } else {
            assert_eq!(patched, upstream);
            assert_eq!(message, None);
        }

        fs::write(buildroot.join("package/pkg-utils.mk"), "something else\n").expect("other");
        let message = apply_reflink_finalize(&buildroot, &output).expect("unknown text");
        assert!(message.is_some_and(|message| message.contains("left as is")));
        let _ = fs::remove_dir_all(buildroot);
        let _ = fs::remove_dir_all(output);
    }
}
