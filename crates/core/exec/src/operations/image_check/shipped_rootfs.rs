//! The kernel modules and firmware of the root filesystem image the build
//! ships, rather than of Buildroot's `target/`: a post-image script that
//! rebuilds the image (from `rootfs.tar`, say) can add or restore files that
//! `target/` does not have. The image is the first required
//! `[[image.expected_images]]` entry in a format read here (erofs, ext2/3/4,
//! squashfs, tar); only its `lib/modules` and `lib/firmware` (under `usr/`
//! with a merged /usr) are extracted, with the Buildroot host tools when the
//! build made them, else the ones on `PATH`. An `fsck.erofs` older than 1.9
//! extracts no single path, so the whole image is extracted then.

use gaia_spec::{BuildrootExpectedImageFormatSpec as Format, ImageDefinition, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory, inside the Buildroot output, the image's trees are
/// extracted to; removed before and after each check.
pub(super) const EXTRACT_DIR: &str = ".gaia-image-check-rootfs";

/// The shipped image: its name in `images/` and its format.
pub(super) fn shipped_image(spec: &ResolvedBuildSpec) -> Option<(String, Format)> {
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        return None;
    };
    buildroot
        .expected_images
        .iter()
        .filter(|image| image.required)
        .find(|image| readable(&image.format))
        .map(|image| (image.name.clone(), image.format.clone()))
}

fn readable(format: &Format) -> bool {
    matches!(
        format,
        Format::Erofs | Format::Ext2 | Format::Ext3 | Format::Ext4 | Format::Squashfs | Format::Tar
    )
}

/// The trees of a shipped image: `lib/modules` and `lib/firmware` of the
/// extracted root (either may be missing).
pub(super) struct ShippedTrees {
    pub modules_root: PathBuf,
    pub firmware_dir: PathBuf,
}

/// Extracts `lib/modules` and `lib/firmware` of `image` into `dest` (which
/// must not exist). An error names what could not be read.
pub(super) fn extract(
    image: &Path,
    format: &Format,
    host_dir: &Path,
    dest: &Path,
) -> Result<ShippedTrees, String> {
    if matches!(format, Format::Erofs) && !erofs_tool(host_dir).1 {
        return extract_whole_erofs(image, host_dir, dest);
    }
    std::fs::create_dir_all(dest)
        .map_err(|error| format!("cannot create '{}': {error}", dest.display()))?;
    // Merged /usr first: there lib is a link to usr/lib.
    let mut lib = None;
    for prefix in ["usr/lib", "lib"] {
        if extract_dir(image, format, host_dir, dest, &format!("{prefix}/modules"))? {
            lib = Some(prefix);
            break;
        }
    }
    let Some(lib) = lib else {
        return Err(format!(
            "'{}' has no lib/modules or usr/lib/modules",
            image.display()
        ));
    };
    extract_dir(image, format, host_dir, dest, &format!("{lib}/firmware"))?;
    Ok(ShippedTrees {
        modules_root: dest.join(lib).join("modules"),
        firmware_dir: dest.join(lib).join("firmware"),
    })
}

/// The whole erofs image extracted to `dest`, for an `fsck.erofs` without
/// `--path`.
fn extract_whole_erofs(image: &Path, host_dir: &Path, dest: &Path) -> Result<ShippedTrees, String> {
    let (tool, _) = erofs_tool(host_dir);
    let output = Command::new(&tool)
        .arg(format!("--extract={}", dest.display()))
        .arg("--no-preserve")
        .arg(image)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", tool.display()))?;
    if !output.status.success() {
        return Err(format!(
            "cannot extract '{}': {}",
            image.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let lib = ["usr/lib", "lib"]
        .into_iter()
        .find(|lib| {
            std::fs::symlink_metadata(dest.join(lib).join("modules"))
                .is_ok_and(|meta| meta.is_dir())
        })
        .ok_or_else(|| {
            format!(
                "'{}' has no lib/modules or usr/lib/modules",
                image.display()
            )
        })?;
    Ok(ShippedTrees {
        modules_root: dest.join(lib).join("modules"),
        firmware_dir: dest.join(lib).join("firmware"),
    })
}

/// The `fsck.erofs` to run (the host tree's, else `PATH`'s, preferring one
/// with `--path`), and whether it has `--path`.
fn erofs_tool(host_dir: &Path) -> (PathBuf, bool) {
    let has_path = |tool: &Path| {
        Command::new(tool)
            .arg("--help")
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout).contains("--path=")
                    || String::from_utf8_lossy(&output.stderr).contains("--path=")
            })
    };
    let host = tool(host_dir, "fsck.erofs");
    if has_path(&host) {
        return (host, true);
    }
    let path = PathBuf::from("fsck.erofs");
    if has_path(&path) {
        return (path, true);
    }
    (host, false)
}

/// Extracts the directory `rel` of the image to `dest/rel`. `Ok(false)` when
/// the image has no such directory.
fn extract_dir(
    image: &Path,
    format: &Format,
    host_dir: &Path,
    dest: &Path,
    rel: &str,
) -> Result<bool, String> {
    let target = dest.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create '{}': {error}", parent.display()))?;
    }
    let mut command = match format {
        Format::Erofs => {
            let mut command = Command::new(erofs_tool(host_dir).0);
            command
                .arg(format!("--extract={}", target.display()))
                .arg(format!("--path=/{rel}"))
                .arg("--no-preserve")
                .arg(image);
            command
        }
        Format::Ext2 | Format::Ext3 | Format::Ext4 => {
            let mut command = Command::new(tool(host_dir, "debugfs"));
            command
                .arg("-R")
                .arg(format!(
                    "rdump /{rel} {}",
                    target.parent().unwrap_or(dest).display()
                ))
                .arg(image);
            command
        }
        Format::Squashfs => {
            let mut command = Command::new(tool(host_dir, "unsquashfs"));
            command
                .arg("-n")
                .arg("-f")
                .arg("-d")
                .arg(dest)
                .arg(image)
                .arg(rel);
            command
        }
        _ => {
            let mut command = Command::new("tar");
            command
                .arg("-xf")
                .arg(image)
                .arg("-C")
                .arg(dest)
                .arg("--wildcards")
                .arg(format!("./{rel}"))
                .arg(format!("./{rel}/*"));
            command
        }
    };
    let output = command
        .output()
        .map_err(|error| format!("cannot run {:?}: {error}", command.get_program()))?;
    // debugfs reports a missing path on stderr and exits 0; the others fail.
    if target.is_dir() {
        return Ok(true);
    }
    if output.status.success() || missing_path(format, &output.stderr) {
        return Ok(false);
    }
    Err(format!(
        "cannot read /{rel} of '{}': {}",
        image.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Whether a failed extraction only says the path is not in the image.
fn missing_path(format: &Format, stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    match format {
        Format::Erofs => stderr.contains("failed to lookup"),
        Format::Squashfs => true,
        Format::Tar => stderr.contains("Not found in archive"),
        _ => false,
    }
}

/// `name` from the Buildroot host tree, else from `PATH`.
fn tool(host_dir: &Path, name: &str) -> PathBuf {
    ["bin", "sbin"]
        .iter()
        .map(|dir| host_dir.join(dir).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    fn put(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, rel).expect("write");
    }

    /// A merged-/usr root: lib links to usr/lib.
    fn root(scratch: &Path) -> PathBuf {
        let root = scratch.join("root");
        put(&root, "usr/lib/modules/7.2.9/kernel/a.ko");
        put(&root, "usr/lib/modules/7.2.9/updates/a.ko");
        put(&root, "usr/lib/firmware/rtl_nic/x.fw");
        put(&root, "usr/bin/tool");
        std::os::unix::fs::symlink("usr/lib", root.join("lib")).expect("lib link");
        root
    }

    fn assert_trees(trees: &ShippedTrees, dest: &Path) {
        assert_eq!(trees.modules_root, dest.join("usr/lib/modules"));
        assert!(trees.modules_root.join("7.2.9/updates/a.ko").is_file());
        assert!(trees.modules_root.join("7.2.9/kernel/a.ko").is_file());
        assert!(trees.firmware_dir.join("rtl_nic/x.fw").is_file());
        assert!(!dest.join("usr/bin").exists(), "only the two trees");
    }

    #[test]
    fn reads_modules_and_firmware_of_a_tar_rootfs() {
        let scratch = TempDir::new("gaia-shipped-tar");
        let root = root(scratch.path());
        let image = scratch.join("rootfs.tar");
        let status = Command::new("tar")
            .arg("-cf")
            .arg(&image)
            .arg("-C")
            .arg(&root)
            .arg(".")
            .status()
            .expect("tar");
        assert!(status.success());
        let dest = scratch.join("out");
        let trees = extract(&image, &Format::Tar, &scratch.join("host"), &dest).expect("extract");
        assert_trees(&trees, &dest);
    }

    #[test]
    fn reads_modules_and_firmware_of_an_erofs_rootfs() {
        let scratch = TempDir::new("gaia-shipped-erofs");
        let root = root(scratch.path());
        let image = scratch.join("rootfs.erofs");
        let Ok(status) = Command::new("mkfs.erofs")
            .arg(&image)
            .arg(&root)
            .stdout(std::process::Stdio::null())
            .status()
        else {
            eprintln!("mkfs.erofs is not installed: skipped");
            return;
        };
        assert!(status.success());
        let dest = scratch.join("out");
        let trees = extract(&image, &Format::Erofs, &scratch.join("host"), &dest).expect("extract");
        assert_trees(&trees, &dest);

        // Without --path (erofs-utils < 1.9) the whole image is extracted.
        let whole = scratch.join("whole");
        let trees = extract_whole_erofs(&image, &scratch.join("host"), &whole).expect("whole");
        assert_eq!(trees.modules_root, whole.join("usr/lib/modules"));
        assert!(trees.modules_root.join("7.2.9/updates/a.ko").is_file());
        assert!(trees.firmware_dir.join("rtl_nic/x.fw").is_file());
    }

    #[test]
    fn an_image_without_modules_is_an_error() {
        let scratch = TempDir::new("gaia-shipped-empty");
        let root = scratch.join("root");
        put(&root, "etc/hostname");
        let image = scratch.join("rootfs.tar");
        assert!(
            Command::new("tar")
                .arg("-cf")
                .arg(&image)
                .arg("-C")
                .arg(&root)
                .arg(".")
                .status()
                .expect("tar")
                .success()
        );
        let error = extract(
            &image,
            &Format::Tar,
            &scratch.join("host"),
            &scratch.join("out"),
        )
        .err()
        .expect("no modules");
        assert!(error.contains("no lib/modules"), "{error}");
    }
}
