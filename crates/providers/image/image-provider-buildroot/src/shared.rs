//! Shared Buildroot output trees (`[providers.buildroot] shared_output`).
//!
//! Builds whose Buildroot inputs are identical compile packages once, in a
//! tree keyed by a digest of those inputs. Each build keeps a private view at
//! its usual `buildroot-output` path:
//!
//! - `build`, `host`, `staging` and `per-package` are symlinks into the
//!   shared tree (read by assembly, sysroot-using artifacts and post-image
//!   scripts);
//! - `.config` is a copy;
//! - `target/` and `images/` are private copy-on-write clones taken from the
//!   shared tree after `make`, so the image feed of one build never reaches
//!   the shared tree or another build's image.
//!
//! The per-build root filesystem images are packed from the private target
//! with the fakeroot scripts Buildroot generated for the shared tree, with
//! their target and images paths rewritten. After the first full `make`, the
//! shared tree only runs `make target-finalize`, so every build packs each
//! filesystem exactly once.
//!
//! All of this happens under an exclusive lock on the shared tree.
use super::*;
use sha2::{Digest, Sha256};
use std::fs::TryLockError;

mod pack;

pub(crate) use pack::{pack_private_rootfs, run_private_post_image};

const SHARED_KEY_VERSION: &str = "gaia-buildroot-shared-output-v1";
const DEFAULT_SHARED_OUTPUT_DIR: &str = ".gaia/cache/buildroot/shared";
const SHARED_POINTER_FILE: &str = ".gaia-shared-output";
const SHARED_PACK_STATE_FILE: &str = ".gaia-shared-pack-state";
const SHARED_KEY_FILE: &str = ".gaia-shared-key.txt";
const PRIVATE_PACK_DIR: &str = ".gaia-pack";
const TARGET_DIR_WARNING_FILE: &str = "THIS_IS_NOT_YOUR_ROOT_FILESYSTEM";
const SHARED_VIEW_ENTRIES: &[&str] = &["build", "host", "staging", "per-package"];
const LOCK_POLL: Duration = Duration::from_millis(500);

/// Root filesystem types that can be packed per build from the shared
/// tree's fakeroot scripts, as (`.config` symbol, `build/buildroot-fs` dir).
const SHARED_ROOTFS_TYPES: &[(&str, &str)] = &[
    ("BR2_TARGET_ROOTFS_BTRFS", "btrfs"),
    ("BR2_TARGET_ROOTFS_CPIO", "cpio"),
    ("BR2_TARGET_ROOTFS_CRAMFS", "cramfs"),
    ("BR2_TARGET_ROOTFS_EROFS", "erofs"),
    ("BR2_TARGET_ROOTFS_EXT2", "ext2"),
    ("BR2_TARGET_ROOTFS_F2FS", "f2fs"),
    ("BR2_TARGET_ROOTFS_JFFS2", "jffs2"),
    ("BR2_TARGET_ROOTFS_ROMFS", "romfs"),
    ("BR2_TARGET_ROOTFS_SQUASHFS", "squashfs"),
    ("BR2_TARGET_ROOTFS_TAR", "tar"),
    ("BR2_TARGET_ROOTFS_UBIFS", "ubifs"),
    // Buildroot 2026.08+: generated from the target tree like ext2.
    ("BR2_TARGET_ROOTFS_XFS", "xfs"),
];

/// Image types whose generation depends on more than the target tree (an
/// initramfs is linked into the kernel; UBI and ISO images embed other
/// images through generated configuration files).
const UNSUPPORTED_SHARED_ROOTFS_TYPES: &[&str] = &[
    "BR2_TARGET_ROOTFS_INITRAMFS",
    "BR2_TARGET_ROOTFS_UBI",
    "BR2_TARGET_ROOTFS_ISO9660",
    "BR2_TARGET_ROOTFS_AXFS",
    "BR2_TARGET_ROOTFS_CLOOP",
    "BR2_TARGET_ROOTFS_OCI",
    "BR2_TARGET_ROOTFS_YAFFS2",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedBuildrootOutput {
    pub(crate) key: String,
    pub(crate) root: PathBuf,
    pub(crate) dir: PathBuf,
    key_material: String,
}

pub(crate) fn shared_buildroot_output(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    buildroot_dir: &Path,
    policy: &ImageExecutionPolicy,
    execution: &ImageExecutionContext,
) -> Result<SharedBuildrootOutput, ImageProviderError> {
    let key_material = shared_output_key_material(spec, image, buildroot_dir, policy, execution)?;
    let mut hasher = Sha256::new();
    hasher.update(key_material.as_bytes());
    let key = hasher
        .finalize()
        .iter()
        .take(10)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let raw_root = policy
        .shared_output_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
        .unwrap_or(DEFAULT_SHARED_OUTPUT_DIR);
    let root = if Path::new(raw_root).is_absolute() {
        PathBuf::from(raw_root)
    } else {
        Path::new(&spec.workspace.root_dir).join(raw_root)
    };
    fs::create_dir_all(&root).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create shared Buildroot output dir '{}': {error}",
                root.display()
            ),
        )
    })?;
    let root = fs::canonicalize(&root).unwrap_or(root);
    Ok(SharedBuildrootOutput {
        dir: root.join(&key),
        key,
        root,
        key_material,
    })
}

/// Everything that decides what the shared tree compiles. The build name and
/// build directory are deliberately not part of it.
pub(crate) fn shared_output_key_material(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    buildroot_dir: &Path,
    policy: &ImageExecutionPolicy,
    execution: &ImageExecutionContext,
) -> Result<String, ImageProviderError> {
    let mut material = format!("{SHARED_KEY_VERSION}\n");
    material.push_str(&format!(
        "source={}\n",
        buildroot_source_identity(buildroot_dir)
    ));
    if let ImageDefinition::Buildroot(buildroot) = &image.definition {
        if let Some(defconfig) = &buildroot.defconfig {
            material.push_str(&format!("defconfig={defconfig}\n"));
        }
        if let Some(defconfig_path) = &buildroot.defconfig_path {
            let resolved = resolve_workspace_path(spec, defconfig_path)?;
            // Content, not location: an import-source checkout's path holds
            // its rev, which must not start a new tree on its own.
            material.push_str(&format!(
                "defconfig_path={}\n",
                file_sha256_or_placeholder(&resolved)
            ));
        }
        for fragment in &buildroot.config_fragments {
            let resolved = resolve_workspace_path(spec, fragment)?;
            material.push_str(&format!(
                "fragment={}\n",
                file_sha256_or_placeholder(&resolved)
            ));
        }
        for (key, value) in normalize_buildroot_config_overrides(spec, &buildroot.config_overrides)
        {
            material.push_str(&format!("override={key}={value}\n"));
        }
        if let Some(external_tree) =
            buildroot_external_tree_value(spec, buildroot.external_tree.as_deref(), None)
        {
            material.push_str(&format!("external_tree={external_tree}\n"));
        }
    }
    let package_dirs = buildroot_package_override_dirs(spec);
    if !package_dirs.is_empty() {
        material.push_str(&format!(
            "package_overrides={}\n",
            package_override_content_digest(&package_dirs)
        ));
    }
    material.push_str(&format!("ccache={}\n", policy.ccache_enabled));
    // Only part of the key when on, so existing shared trees keep theirs.
    if policy.parallel_packages {
        material.push_str("parallel_packages=true\n");
    }
    material.push_str(&format!(
        "docker_image={}\n",
        execution.docker_image.as_deref().unwrap_or_default()
    ));
    Ok(material)
}

/// Content identity recorded by the source provider, without the per-build
/// fields (build version, profile, ...) of the same state file.
fn buildroot_source_identity(buildroot_dir: &Path) -> String {
    const IDENTITY_KEYS: &[&str] = &[
        "materialized_tree_digest",
        "extracted_tree_digest",
        "archive_sha256",
        "path_digest",
    ];
    if let Ok(contents) = fs::read_to_string(buildroot_dir.join(".gaia-source-state.txt")) {
        let state = gaia_spec::KeyValueState::parse(&contents).into_map();
        if let Some(commit) = state
            .get("resolved_commit_sha")
            .filter(|commit| !commit.trim().is_empty())
        {
            return format!("commit={commit}");
        }
        let identity = IDENTITY_KEYS
            .iter()
            .filter_map(|key| state.get(*key).map(|value| format!("{key}={value}")))
            .collect::<Vec<_>>();
        if !identity.is_empty() {
            return identity.join(";");
        }
    }
    let canonical = fs::canonicalize(buildroot_dir).unwrap_or_else(|_| buildroot_dir.into());
    format!("path={}", canonical.display())
}

/// An exclusive lock on one shared tree, released on drop.
pub(crate) struct SharedOutputLock {
    _file: fs::File,
}

pub(crate) fn lock_shared_output(
    shared: &SharedBuildrootOutput,
    cancel_check: Option<&ProcessCancelCheck>,
    log_sink: Option<&ProcessLogSink>,
) -> Result<SharedOutputLock, ImageProviderError> {
    let lock_path = shared.root.join(format!("{}.lock", shared.key));
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to open shared Buildroot lock '{}': {error}",
                    lock_path.display()
                ),
            )
        })?;
    let mut announced = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(SharedOutputLock { _file: file }),
            Err(TryLockError::WouldBlock) => {
                if !announced {
                    announced = true;
                    let message = format!(
                        "waiting for another build using shared Buildroot tree '{}'",
                        shared.dir.display()
                    );
                    tracing::info!(shared_tree = %shared.dir.display(), "{message}");
                    if let Some(log_sink) = log_sink {
                        log_sink(gaia_process::ProcessLogLine {
                            stream: gaia_process::ProcessLogStream::Stderr,
                            line: message,
                        });
                    }
                }
                if cancel_check.is_some_and(|cancel| cancel()) {
                    return Err(ImageProviderError::new(
                        ImageProviderErrorKind::Cancelled,
                        "cancelled while waiting for the shared Buildroot tree lock",
                    ));
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::Error(error)) => {
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to lock shared Buildroot tree '{}': {error}",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
}

/// Root filesystem types enabled in `config` that are packed per build.
pub(crate) fn shared_rootfs_types(config: &str) -> Result<Vec<&'static str>, ImageProviderError> {
    let enabled = |symbol: &str| {
        let assignment = format!("{symbol}=y");
        config.lines().any(|line| line.trim() == assignment)
    };
    if let Some(unsupported) = UNSUPPORTED_SHARED_ROOTFS_TYPES
        .iter()
        .find(|symbol| enabled(symbol))
    {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "providers.buildroot.shared_output does not support {unsupported}; disable shared_output for this build"
            ),
        ));
    }
    Ok(SHARED_ROOTFS_TYPES
        .iter()
        .filter(|(symbol, _)| enabled(symbol))
        .map(|(_, dir)| *dir)
        .collect())
}

fn config_file_sha(output_dir: &Path) -> Option<String> {
    let config = output_dir.join(".config");
    config
        .is_file()
        .then(|| file_sha256_or_placeholder(&config))
}

/// Whether the fakeroot scripts of the last full `make` match the current
/// `.config`, so `make target-finalize` is enough for this run.
pub(crate) fn shared_pack_scripts_current(output_dir: &Path, fs_types: &[&str]) -> bool {
    let Some(config_sha) = config_file_sha(output_dir) else {
        return false;
    };
    fs::read_to_string(output_dir.join(SHARED_PACK_STATE_FILE))
        .is_ok_and(|state| state.trim() == config_sha)
        && fs_types.iter().all(|fs_type| {
            output_dir
                .join("build/buildroot-fs")
                .join(fs_type)
                .join("fakeroot")
                .is_file()
        })
}

pub(crate) fn clear_shared_pack_state(output_dir: &Path) -> Result<(), ImageProviderError> {
    remove_path_if_exists(&output_dir.join(SHARED_PACK_STATE_FILE))
}

pub(crate) fn write_shared_pack_state(output_dir: &Path) -> Result<(), ImageProviderError> {
    let Some(config_sha) = config_file_sha(output_dir) else {
        return Ok(());
    };
    write_small_file(
        &output_dir.join(SHARED_PACK_STATE_FILE),
        &format!("{config_sha}\n"),
    )
}

fn write_small_file(path: &Path, contents: &str) -> Result<(), ImageProviderError> {
    fs::write(path, contents).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to write '{}': {error}", path.display()),
        )
    })
}

/// A build output dir that holds a complete private Buildroot tree.
fn is_private_tree(output_dir: &Path) -> bool {
    fs::symlink_metadata(output_dir.join("build")).is_ok_and(|metadata| metadata.is_dir())
}

/// A build output dir that is a view onto a shared tree.
pub(crate) fn is_shared_view(output_dir: &Path) -> bool {
    output_dir.join(SHARED_POINTER_FILE).is_file()
        || fs::symlink_metadata(output_dir.join("build"))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Called when shared output is disabled: a private tree must never be built
/// through symlinks into a shared tree, so the view is removed and this build
/// stops holding the shared tree.
pub(crate) fn leave_shared_view(output_dir: &Path) -> Result<Vec<String>, ImageProviderError> {
    if !is_shared_view(output_dir) {
        return Ok(Vec::new());
    }
    let mut messages = release_shared_user(output_dir)?;
    remove_path_if_exists(output_dir)?;
    messages.push(format!(
        "removed shared Buildroot view '{}' because shared_output is disabled",
        output_dir.display()
    ));
    Ok(messages)
}

pub(crate) struct SharedBuildRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) shared: &'a SharedBuildrootOutput,
    pub(crate) command: ImageCommandContext<'a>,
}

/// Prepare operation: bring the shared tree up to date and expose it (host,
/// staging, a private target) at the build's output dir.
pub(crate) fn prepare_with_shared_output(
    request: SharedBuildRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let _lock = lock_shared_output(
        request.shared,
        request.command.cancel_check.as_ref(),
        request.command.log_sink.as_ref(),
    )?;
    let mut messages = make_shared_tree(&request)?;
    messages.extend(materialize_shared_view(&request)?);
    Ok(messages)
}

/// Build operation: shared make, private target with the image feed, one
/// pack per root filesystem type, and post-image scripts on the private
/// images dir.
pub(crate) fn build_with_shared_output(
    request: SharedBuildRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let _lock = lock_shared_output(
        request.shared,
        request.command.cancel_check.as_ref(),
        request.command.log_sink.as_ref(),
    )?;
    let mut messages = make_shared_tree(&request)?;
    messages.extend(materialize_shared_view(&request)?);

    let SharedBuildRequest {
        spec,
        image,
        buildroot_dir,
        output_dir,
        shared,
        command,
    } = request;
    let target_dir = output_dir.join("target");
    if image_feed_has_content(image) {
        let signature = build_image_feed_signature(spec, image)?;
        apply_image_feed_to_rootfs(spec, image, &target_dir)?;
        write_image_feed_managed_paths(output_dir, spec, image)?;
        write_image_feed_signature(output_dir, &signature)?;
        messages.push("applied image feed to the private Buildroot target".into());
    } else {
        remove_path_if_exists(&image_feed_signature_path(output_dir))?;
        remove_path_if_exists(&image_feed_managed_paths_path(output_dir))?;
    }

    let config = fs::read_to_string(shared.dir.join(".config")).unwrap_or_default();
    let fs_types = shared_rootfs_types(&config)?;
    messages.extend(pack_private_rootfs(
        &shared.dir,
        output_dir,
        buildroot_dir,
        &fs_types,
        command.clone(),
    )?);
    messages.extend(run_private_post_image(
        &shared.dir,
        output_dir,
        buildroot_dir,
        command.clone(),
    )?);
    if image_feed_has_content(image) {
        refresh_expected_tar_images(image, &target_dir, output_dir, command.execution)?;
    }
    Ok(messages)
}

fn make_shared_tree(request: &SharedBuildRequest<'_>) -> Result<Vec<String>, ImageProviderError> {
    let mut messages = Vec::new();
    if is_private_tree(request.output_dir) {
        remove_path_if_exists(request.output_dir)?;
        messages.push(format!(
            "removed private Buildroot tree '{}'; this build now uses shared tree '{}'",
            request.output_dir.display(),
            request.shared.dir.display()
        ));
    }
    fs::create_dir_all(&request.shared.dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create shared Buildroot tree '{}': {error}",
                request.shared.dir.display()
            ),
        )
    })?;
    write_small_file(
        &request.shared.dir.join(SHARED_KEY_FILE),
        &request.shared.key_material,
    )?;
    messages.extend(run_buildroot_with(
        BuildrootRunRequest {
            spec: request.spec,
            image: request.image,
            buildroot_dir: request.buildroot_dir,
            output_dir: &request.shared.dir,
            command: request.command.clone(),
        },
        BuildrootMakeOptions {
            post_build_script: None,
            shared_tree: true,
        },
    )?);
    messages.push(format!(
        "built shared Buildroot tree '{}' (key {})",
        request.shared.dir.display(),
        request.shared.key
    ));
    Ok(messages)
}

/// Links the shared tree into the build's output dir and takes private
/// clones of `target/` and `images/`.
fn materialize_shared_view(
    request: &SharedBuildRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let SharedBuildRequest {
        output_dir, shared, ..
    } = request;
    let mut messages = Vec::new();
    fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create Buildroot output dir '{}': {error}",
                output_dir.display()
            ),
        )
    })?;
    for entry in SHARED_VIEW_ENTRIES {
        let link = output_dir.join(entry);
        let target = shared.dir.join(entry);
        if fs::read_link(&link).is_ok_and(|current| current == target) {
            continue;
        }
        remove_path_if_exists(&link)?;
        if fs::symlink_metadata(&target).is_ok() {
            symlink_path(&target, &link)?;
        }
    }
    let shared_config = shared.dir.join(".config");
    if shared_config.is_file() {
        fs::copy(&shared_config, output_dir.join(".config")).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy shared Buildroot config into '{}': {error}",
                    output_dir.display()
                ),
            )
        })?;
    }

    let target_dir = output_dir.join("target");
    let shared_target = shared.dir.join("target");
    if shared_target.is_dir() {
        clone_tree(&shared_target, &target_dir)?;
        remove_path_if_exists(&target_dir.join(TARGET_DIR_WARNING_FILE))?;
    } else {
        remove_path_if_exists(&target_dir)?;
    }
    let images_dir = output_dir.join("images");
    let shared_images = shared.dir.join("images");
    if shared_images.is_dir() {
        clone_tree(&shared_images, &images_dir)?;
    } else {
        remove_path_if_exists(&images_dir)?;
    }

    messages.extend(record_shared_user(shared, output_dir)?);
    Ok(messages)
}

#[cfg(unix)]
fn symlink_path(target: &Path, link: &Path) -> Result<(), ImageProviderError> {
    std::os::unix::fs::symlink(target, link).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to link '{}' to shared '{}': {error}",
                link.display(),
                target.display()
            ),
        )
    })
}

#[cfg(not(unix))]
fn symlink_path(target: &Path, link: &Path) -> Result<(), ImageProviderError> {
    Err(ImageProviderError::new(
        ImageProviderErrorKind::PolicyBlocked,
        format!(
            "shared Buildroot output needs symlinks to link '{}' to '{}'",
            link.display(),
            target.display()
        ),
    ))
}

fn user_entry_name(output_dir: &Path) -> String {
    let canonical = fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.into());
    let mut hasher = Sha256::new();
    hasher.update(canonical.display().to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn users_dir(root: &Path, key: &str) -> PathBuf {
    root.join(format!("{key}.users"))
}

/// Registers this build as a user of `shared`, then releases the tree it
/// used before, if any.
fn record_shared_user(
    shared: &SharedBuildrootOutput,
    output_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let pointer = output_dir.join(SHARED_POINTER_FILE);
    let previous = read_shared_pointer(&pointer);
    let mut messages = Vec::new();
    if let Some((previous_root, previous_key)) = &previous
        && (previous_root != &shared.root || previous_key != &shared.key)
    {
        messages.extend(release_shared_tree(previous_root, previous_key, output_dir));
    }
    let users = users_dir(&shared.root, &shared.key);
    fs::create_dir_all(&users).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create shared Buildroot users dir '{}': {error}",
                users.display()
            ),
        )
    })?;
    write_small_file(
        &users.join(user_entry_name(output_dir)),
        &format!("{}\n", output_dir.display()),
    )?;
    write_small_file(
        &pointer,
        &format!("{}\n{}\n", shared.root.display(), shared.key),
    )?;
    Ok(messages)
}

fn read_shared_pointer(pointer: &Path) -> Option<(PathBuf, String)> {
    let contents = fs::read_to_string(pointer).ok()?;
    let mut lines = contents.lines();
    let root = PathBuf::from(lines.next()?.trim());
    let key = lines.next()?.trim().to_string();
    (!key.is_empty()).then_some((root, key))
}

fn release_shared_user(output_dir: &Path) -> Result<Vec<String>, ImageProviderError> {
    let pointer = output_dir.join(SHARED_POINTER_FILE);
    let Some((root, key)) = read_shared_pointer(&pointer) else {
        return Ok(Vec::new());
    };
    let messages = release_shared_tree(&root, &key, output_dir);
    remove_path_if_exists(&pointer)?;
    Ok(messages)
}

/// Drops this build's registration on a shared tree and deletes the tree
/// when no registered build uses it anymore and nobody holds its lock.
/// Failures are reported but never fail the build.
fn release_shared_tree(root: &Path, key: &str, output_dir: &Path) -> Vec<String> {
    let users = users_dir(root, key);
    let _ = fs::remove_file(users.join(user_entry_name(output_dir)));
    let still_used = fs::read_dir(&users).is_ok_and(|mut entries| entries.next().is_some());
    if still_used {
        return Vec::new();
    }
    let lock_path = root.join(format!("{key}.lock"));
    let Ok(lock_file) = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        return Vec::new();
    };
    if lock_file.try_lock().is_err() {
        return Vec::new();
    }
    let tree = root.join(key);
    let result = remove_path_if_exists(&tree).and_then(|()| remove_path_if_exists(&users));
    let _ = fs::remove_file(&lock_path);
    drop(lock_file);
    match result {
        Ok(()) => vec![format!(
            "removed unused shared Buildroot tree '{}'",
            tree.display()
        )],
        Err(error) => vec![format!(
            "failed to remove unused shared Buildroot tree '{}': {}",
            tree.display(),
            error.message
        )],
    }
}

/// Copies a directory tree, sharing extents with the source where the
/// filesystem supports it (`cp --reflink=auto`, e.g. btrfs or XFS), and
/// falling back to a plain recursive copy.
pub(crate) fn clone_tree(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    remove_path_if_exists(dest)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to create '{}': {error}", parent.display()),
            )
        })?;
    }
    let cloned = Command::new("cp")
        .arg("-a")
        .arg("--reflink=auto")
        .arg("--")
        .arg(src)
        .arg(dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if cloned {
        return Ok(());
    }
    remove_path_if_exists(dest)?;
    copy_path(src, dest)?;
    copy_dir_modes(src, dest)
}

/// [`copy_path`] creates directories with default permissions; restore the
/// source modes (for example `/root` 0700 or `/tmp` 1777).
fn copy_dir_modes(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    let Ok(metadata) = fs::symlink_metadata(src) else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    if let Ok(entries) = fs::read_dir(src) {
        for entry in entries.flatten() {
            copy_dir_modes(&entry.path(), &dest.join(entry.file_name()))?;
        }
    }
    fs::set_permissions(dest, metadata.permissions()).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to set permissions on '{}': {error}", dest.display()),
        )
    })
}
