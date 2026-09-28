//! Delivering the image feed through Buildroot's own `make`.
//!
//! The feed is staged in a directory next to the output tree and copied into
//! `target/` by a generated post-build script appended to
//! `BR2_ROOTFS_POST_BUILD_SCRIPT` on the make command line. Post-build
//! scripts run after package installation, target stripping and rootfs
//! overlays and before the filesystem images are generated, so a single
//! `make` packs every image once with the feed included. The `.config` file
//! is not modified, so enabling this never triggers a Buildroot clean.
use super::*;
use sha2::{Digest, Sha256};

const STAGED_FEED_DIR: &str = ".gaia-image-feed";
/// Shell arguments per generated command line.
const SCRIPT_BATCH: usize = 64;

pub(crate) struct StagedImageFeed {
    pub(crate) script: PathBuf,
    marker: PathBuf,
    nonce: String,
    pub(crate) signature: String,
}

impl StagedImageFeed {
    /// Whether Buildroot ran the generated post-build script during the
    /// last make.
    pub(crate) fn applied(&self) -> bool {
        fs::read_to_string(&self.marker).is_ok_and(|contents| contents == self.nonce)
    }
}

pub(crate) fn staged_image_feed_dir(output_dir: &Path) -> PathBuf {
    output_dir.join(STAGED_FEED_DIR)
}

/// Removes stale feed outputs from the existing target tree and stages the
/// current feed for the post-build script. Returns `None` when the image has
/// no feed.
pub(crate) fn stage_image_feed_for_make(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    output_dir: &Path,
) -> Result<Option<StagedImageFeed>, ImageProviderError> {
    let staged_dir = staged_image_feed_dir(output_dir);
    remove_path_if_exists(&staged_dir)?;
    if !image_feed_has_content(image) {
        return Ok(None);
    }
    let target_dir = output_dir.join("target");
    if target_dir.is_dir() {
        prune_stale_image_feed_outputs(spec, image, &target_dir, output_dir)?;
    }
    let signature = build_image_feed_signature(spec, image)?;
    let rootfs_dir = staged_dir.join("rootfs");
    fs::create_dir_all(&rootfs_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create staged image feed dir '{}': {error}",
                rootfs_dir.display()
            ),
        )
    })?;
    apply_image_feed_to_rootfs(spec, image, &rootfs_dir)?;

    let marker = staged_dir.join("applied");
    let nonce = {
        let mut hasher = Sha256::new();
        hasher.update(signature.as_bytes());
        hasher.update(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
                .to_le_bytes(),
        );
        hasher.update(std::process::id().to_le_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let script = staged_dir.join("post-build.sh");
    let body =
        feed_post_build_script(&rootfs_dir, &image_feed_modes(spec, image), &marker, &nonce)?;
    fs::write(&script, body).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write image feed script '{}': {error}",
                script.display()
            ),
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to mark image feed script '{}' executable: {error}",
                script.display()
            ),
        )
    })?;
    Ok(Some(StagedImageFeed {
        script,
        marker,
        nonce,
        signature,
    }))
}

/// Explicit modes declared by install entries and stage files, keyed by
/// their destination in the image.
fn image_feed_modes(spec: &ResolvedBuildSpec, image: &ImageSpec) -> Vec<(String, u32)> {
    let installs = image.feed.install_entries.iter().filter_map(|install_id| {
        let install = spec
            .install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)?;
        Some((install.dest.clone(), install.mode?))
    });
    let stage_files = image.feed.stage_files.iter().filter_map(|stage_file_id| {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)?;
        Some((stage_file.dest.clone(), stage_file.mode?))
    });
    installs.chain(stage_files).collect()
}

/// Generates a script that copies `rootfs_dir` into the target given as
/// `$1` with the same semantics as [`copy_path`]: directories are merged
/// (existing directory modes are kept), and every other entry replaces
/// whatever was at its destination. Existing destinations are removed
/// before copying, so a destination symlink is never followed.
pub(crate) fn feed_post_build_script(
    rootfs_dir: &Path,
    modes: &[(String, u32)],
    marker: &Path,
    nonce: &str,
) -> Result<String, ImageProviderError> {
    let mut dirs = Vec::new();
    let mut entries = BTreeMap::<String, Vec<String>>::new();
    collect_feed_entries(rootfs_dir, "", &mut dirs, &mut entries)?;

    let mut script = String::from(
        "#!/bin/sh\n# Generated by Gaia: copies the image feed into the Buildroot target.\nset -e\nT=\"$1\"\n",
    );
    script.push_str(&format!(
        "F={}\n",
        shell_quote(&rootfs_dir.display().to_string())
    ));
    script.push_str(
        "if [ -z \"$T\" ] || [ ! -d \"$T\" ]; then\n  echo \"gaia image feed: target directory '$T' does not exist\" >&2\n  exit 1\nfi\n",
    );
    for chunk in dirs.chunks(SCRIPT_BATCH) {
        script.push_str("mkdir -p");
        for dir in chunk {
            script.push_str(&format!(" \"$T\"{}", shell_quote(&format!("/{dir}"))));
        }
        script.push('\n');
    }
    let files = entries.values().flatten().collect::<Vec<_>>();
    for chunk in files.chunks(SCRIPT_BATCH) {
        script.push_str("rm -rf --");
        for file in chunk {
            script.push_str(&format!(" \"$T\"{}", shell_quote(&format!("/{file}"))));
        }
        script.push('\n');
    }
    for (parent, names) in &entries {
        let dest = if parent.is_empty() {
            "/".to_string()
        } else {
            format!("/{parent}/")
        };
        for chunk in names.chunks(SCRIPT_BATCH) {
            script.push_str("cp -a --");
            for name in chunk {
                script.push_str(&format!(" \"$F\"{}", shell_quote(&format!("/{name}"))));
            }
            script.push_str(&format!(" \"$T\"{}\n", shell_quote(&dest)));
        }
    }
    for (dest, mode) in modes {
        let dest = format!("/{}", dest.trim_start_matches('/'));
        script.push_str(&format!("chmod {mode:o} \"$T\"{}\n", shell_quote(&dest)));
    }
    script.push_str(&format!(
        "printf '%s' {} > {}\n",
        shell_quote(nonce),
        shell_quote(&marker.display().to_string())
    ));
    Ok(script)
}

fn collect_feed_entries(
    root: &Path,
    relative: &str,
    dirs: &mut Vec<String>,
    entries: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), ImageProviderError> {
    let dir = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let mut children = fs::read_dir(&dir)
        .map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read staged image feed dir '{}': {error}",
                    dir.display()
                ),
            )
        })?
        .map(|entry| {
            entry.map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read staged image feed entry in '{}': {error}",
                        dir.display()
                    ),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().to_string();
        let child_relative = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        let file_type = child.file_type().map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to inspect staged image feed entry '{}': {error}",
                    child.path().display()
                ),
            )
        })?;
        if file_type.is_dir() {
            dirs.push(child_relative.clone());
            collect_feed_entries(root, &child_relative, dirs, entries)?;
        } else {
            entries
                .entry(relative.to_string())
                .or_default()
                .push(child_relative);
        }
    }
    Ok(())
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The value to pass as `BR2_ROOTFS_POST_BUILD_SCRIPT` on the make command
/// line: the configured scripts followed by `script`.
pub(crate) fn post_build_script_override(output_dir: &Path, script: &Path) -> String {
    let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
    let configured =
        buildroot_config_value(&config, "BR2_ROOTFS_POST_BUILD_SCRIPT").unwrap_or_default();
    let mut scripts = configured
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    scripts.push(script.display().to_string());
    format!("BR2_ROOTFS_POST_BUILD_SCRIPT={}", scripts.join(" "))
}
