//! Whether Buildroot's filesystem images and post-image step can be
//! skipped: Buildroot regenerates every rootfs image (EROFS, ext4, squashfs,
//! genimage disks...) and reruns the post-image script on every `make`, even
//! when nothing they read changed. Gaia builds up to `target-finalize`,
//! digests everything the image step reads, and runs the rest of `make` only
//! when that digest differs from the one recorded with the current images.
//!
//! The digest covers the finalized target tree (each file's path, type,
//! mode, size and content; link targets), the images packages installed
//! (kernel, device trees, bootloaders: their `.files-list-images.txt`), the
//! whole `.config`, the files the image settings name (post-image and
//! fakeroot scripts, users and device tables, script arguments) and the
//! directory each sits in (post-image scripts read files next to them),
//! Buildroot's `fs/` and `support/scripts` sources, and the host packages
//! built (their build directories carry their versions).

use super::*;
use gaia_image_providers::{ExternalTree, expand_external_paths};
use sha2::{Digest, Sha256};

const ROOTFS_INPUTS_STATE: &str = ".gaia-rootfs-inputs";

/// Settings whose values name files the image step reads.
const IMAGE_FILE_SETTINGS: &[&str] = &[
    "BR2_ROOTFS_POST_IMAGE_SCRIPT",
    "BR2_ROOTFS_POST_SCRIPT_ARGS",
    "BR2_ROOTFS_POST_FAKEROOT_SCRIPT",
    "BR2_ROOTFS_USERS_TABLES",
    "BR2_ROOTFS_DEVICE_TABLE",
    "BR2_ROOTFS_STATIC_DEVICE_TABLE",
];

/// The digest of everything the filesystem image step reads. `trees` are the
/// build's `BR2_EXTERNAL` trees, which the settings name through
/// `$(BR2_EXTERNAL_<NAME>_PATH)`.
pub(crate) fn rootfs_inputs_digest(
    buildroot_dir: &Path,
    output_dir: &Path,
    trees: &[ExternalTree],
) -> String {
    let mut hasher = Sha256::new();
    hash_target_tree(&output_dir.join("target"), &mut hasher);
    // The packages left out of target/ (image.rootfs.exclude_packages): the
    // images change with them even where the tree listing alone would not.
    let excluded = applied_rootfs_exclude(output_dir);
    if !excluded.is_empty() {
        hasher.update(b"\0rootfs-exclude\0");
        hasher.update(excluded.as_bytes());
    }

    // Images packages installed (kernel, device trees, bootloaders).
    let mut package_images = BTreeSet::new();
    for entry in fs::read_dir(output_dir.join("build"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(list) = fs::read_to_string(entry.path().join(".files-list-images.txt")) else {
            continue;
        };
        package_images.extend(
            list.lines()
                .filter_map(|line| line.split_once(','))
                .map(|(_, path)| path.trim_start_matches("./").to_string()),
        );
    }
    for image in package_images {
        hasher.update(b"image\0");
        hasher.update(image.as_bytes());
        hasher.update(path_content_digest(&output_dir.join("images").join(&image)));
    }

    let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
    hasher.update(b"config\0");
    hasher.update(config.as_bytes());
    for line in config.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !IMAGE_FILE_SETTINGS.contains(&key) {
            continue;
        }
        let expanded =
            expand_setting_value(value.trim_matches('"'), buildroot_dir, output_dir, trees);
        for token in expanded.split_whitespace() {
            let Some(resolved) = resolve_setting_file(token, buildroot_dir, trees) else {
                continue;
            };
            hasher.update(b"file\0");
            hasher.update(resolved.display().to_string().as_bytes());
            hasher.update(path_content_digest(&resolved));
            if let Some(parent) = resolved.parent() {
                hasher.update(path_content_digest(parent));
            }
        }
    }

    for tool_dir in ["fs", "support/scripts"] {
        hasher.update(tool_dir.as_bytes());
        hasher.update(path_content_digest(&buildroot_dir.join(tool_dir)));
    }
    let mut host_packages = fs::read_dir(output_dir.join("build"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with("host-"))
        .collect::<Vec<_>>();
    host_packages.sort();
    hasher.update(host_packages.join("\0").as_bytes());
    hex(&hasher.finalize())
}

/// Expands the variables an image setting may use, as Buildroot does when it
/// runs the script: the external trees (`$(BR2_EXTERNAL_<NAME>_PATH)`), the
/// Buildroot tree (`$(TOPDIR)`), the output dir (`$(BASE_DIR)`) and the
/// `.config` (`$(BR2_CONFIG)`).
fn expand_setting_value(
    value: &str,
    buildroot_dir: &Path,
    output_dir: &Path,
    trees: &[ExternalTree],
) -> String {
    expand_external_paths(value, trees)
        .replace("$(TOPDIR)", &buildroot_dir.display().to_string())
        .replace("$(BASE_DIR)", &output_dir.display().to_string())
        .replace(
            "$(BR2_CONFIG)",
            &output_dir.join(".config").display().to_string(),
        )
}

/// The file a setting token names: an absolute path as it is, a relative one
/// from the Buildroot tree, then from the external trees. A token that still
/// holds a `$(...)` variable, or names no file, is `None`.
fn resolve_setting_file(
    token: &str,
    buildroot_dir: &Path,
    trees: &[ExternalTree],
) -> Option<PathBuf> {
    if token.contains('$') {
        return None;
    }
    let path = Path::new(token);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::iter::once(buildroot_dir)
        .chain(trees.iter().map(|tree| tree.dir.as_path()))
        .map(|base| base.join(path))
        .find(|candidate| candidate.is_file())
}

/// Every entry of the target tree, in sorted order: path, type, mode,
/// size, and content (files) or target (links).
fn hash_target_tree(root: &Path, hasher: &mut Sha256) {
    fn visit(root: &Path, dir: &Path, hasher: &mut Sha256) {
        let mut entries = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let relative = path.strip_prefix(root).unwrap_or(&path);
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(metadata.permissions().mode().to_le_bytes());
            if metadata.file_type().is_symlink() {
                hasher.update(b"\0link\0");
                if let Ok(target) = fs::read_link(&path) {
                    hasher.update(target.to_string_lossy().as_bytes());
                }
            } else if metadata.is_dir() {
                hasher.update(b"\0dir\0");
                visit(root, &path, hasher);
            } else {
                hasher.update(b"\0file\0");
                hasher.update(metadata.len().to_le_bytes());
                hasher.update(file_sha256_or_placeholder(&path).as_bytes());
            }
            hasher.update(b"\0");
        }
    }
    visit(root, root, hasher);
}

/// The digest recorded with the current images, if any.
pub(crate) fn recorded_rootfs_inputs(output_dir: &Path) -> Option<String> {
    fs::read_to_string(output_dir.join(ROOTFS_INPUTS_STATE))
        .ok()
        .map(|digest| digest.trim().to_string())
}

pub(crate) fn record_rootfs_inputs(
    output_dir: &Path,
    digest: Option<&str>,
) -> Result<(), ImageProviderError> {
    let path = output_dir.join(ROOTFS_INPUTS_STATE);
    match digest {
        Some(digest) => fs::write(&path, digest),
        None => match fs::remove_file(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to record rootfs image inputs in '{}': {error}",
            path.display()
        ))
    })
}

/// Runs the filesystem images and post-image step (`images_command`)
/// unless their inputs match the ones recorded with the current images.
pub(crate) fn run_images_if_inputs_changed(
    images_command: Command,
    image: &ImageSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    trees: &[ExternalTree],
    command_context: &ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let started = std::time::Instant::now();
    let digest = rootfs_inputs_digest(buildroot_dir, output_dir, trees);
    let mut messages = vec![gaia_process::step_time_message(
        "rootfs image inputs digest",
        started.elapsed(),
    )];
    if recorded_rootfs_inputs(output_dir).as_deref() == Some(digest.as_str())
        && expected_images_present(image, output_dir)
    {
        messages.push(
            "skipped buildroot filesystem images and post-image: their inputs are unchanged"
                .to_string(),
        );
        return Ok(messages);
    }
    // The images make applies the image feed to `target/`: the tree is no
    // longer the finalized one.
    invalidate_finalized(output_dir);
    // Not current until the images are written again.
    record_rootfs_inputs(output_dir, None)?;
    messages.extend(run_command(
        images_command,
        "buildroot images",
        command_context.execution,
        command_context.policy,
        command_context.log_sink.clone(),
        command_context.cancel_check.clone(),
    )?);
    record_rootfs_inputs(output_dir, Some(&digest))?;
    Ok(messages)
}

/// Whether every required expected image exists (or, with none declared,
/// any image does), so skipping the image step leaves usable images.
fn expected_images_present(image: &ImageSpec, output_dir: &Path) -> bool {
    let images = output_dir.join("images");
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) if !buildroot.expected_images.is_empty() => buildroot
            .expected_images
            .iter()
            .filter(|expected| expected.required)
            .all(|expected| images.join(&expected.name).is_file()),
        _ => fs::read_dir(&images).is_ok_and(|mut entries| entries.next().is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn the_digest_follows_target_contents_package_images_and_image_scripts() {
        let root_scratch = TempDir::new("gaia-rootfs-inputs");
        let root = root_scratch.join("root");
        let buildroot = root.join("buildroot");
        let output = root.join("output");
        for (path, contents) in [
            (output.join("target/usr/bin/app"), "app"),
            (output.join("images/Image"), "kernel"),
            (output.join("images/rootfs.erofs"), "generated"),
            (
                output.join("build/linux-6/.files-list-images.txt"),
                "linux,./Image\n",
            ),
            (buildroot.join("board/post-image.sh"), "genimage"),
            (buildroot.join("board/genimage.cfg"), "image sdcard.img {}"),
            (buildroot.join("fs/erofs/erofs.mk"), "mk"),
        ] {
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, contents).expect("file");
        }
        fs::write(
            output.join(".config"),
            "BR2_TARGET_ROOTFS_EROFS=y\nBR2_ROOTFS_POST_IMAGE_SCRIPT=\"board/post-image.sh\"\n",
        )
        .expect("config");
        let digest = || rootfs_inputs_digest(&buildroot, &output, &[]);
        let first = digest();
        assert_eq!(first, digest());

        // The images the image step itself writes do not count.
        fs::write(output.join("images/rootfs.erofs"), "regenerated").expect("image");
        assert_eq!(first, digest());

        for (path, contents) in [
            (output.join("target/usr/bin/app"), "app v2"),
            (output.join("images/Image"), "kernel v2"),
            (
                buildroot.join("board/genimage.cfg"),
                "image sdcard.img { size = 8G }",
            ),
        ] {
            let before = digest();
            fs::write(&path, contents).expect("change");
            assert_ne!(before, digest(), "{}", path.display());
        }

        record_rootfs_inputs(&output, Some("abc")).expect("record");
        assert_eq!(recorded_rootfs_inputs(&output).as_deref(), Some("abc"));
        record_rootfs_inputs(&output, None).expect("forget");
        assert_eq!(recorded_rootfs_inputs(&output), None);
    }

    #[test]
    fn the_digest_follows_image_scripts_named_through_buildroot_variables() {
        let root_scratch = TempDir::new("gaia-rootfs-inputs-variables");
        let root = root_scratch.join("root");
        let buildroot = root.join("buildroot");
        let output = root.join("output");
        let external = root.join("board-tree");
        for (path, contents) in [
            (output.join("target/usr/bin/app"), "app"),
            (output.join("images/rootfs.erofs"), "generated"),
            (external.join("board/post-image.sh"), "genimage v1"),
            (output.join("board/genimage.cfg"), "image sdcard.img {}"),
            (buildroot.join("fs/erofs/erofs.mk"), "mk"),
        ] {
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, contents).expect("file");
        }
        // The script lives in an external tree; the arguments name a file
        // through `$(BASE_DIR)`, which Buildroot expands before running it.
        fs::write(
            output.join(".config"),
            "BR2_TARGET_ROOTFS_EROFS=y\n\
             BR2_ROOTFS_POST_IMAGE_SCRIPT=\"$(BR2_EXTERNAL_BOARD_PATH)/board/post-image.sh\"\n\
             BR2_ROOTFS_POST_SCRIPT_ARGS=\"-c $(BASE_DIR)/board/genimage.cfg\"\n",
        )
        .expect("config");
        let trees = vec![ExternalTree {
            name: "board".to_string(),
            dir: external.clone(),
        }];
        let digest = || rootfs_inputs_digest(&buildroot, &output, &trees);
        let first = digest();

        fs::write(external.join("board/post-image.sh"), "genimage v2").expect("edit script");
        let after_script = digest();
        assert_ne!(first, after_script, "an edited external post-image script");

        fs::write(
            output.join("board/genimage.cfg"),
            "image sdcard.img { size = 8G }",
        )
        .expect("edit cfg");
        assert_ne!(
            after_script,
            digest(),
            "an edited $(BASE_DIR) script argument"
        );
    }
}
