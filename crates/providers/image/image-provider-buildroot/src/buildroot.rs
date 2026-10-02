use super::*;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(crate) fn resolve_buildroot_dir(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Option<PathBuf> {
    if let Some(source_id) = buildroot_source_id(image) {
        let candidate = buildroot_source_dir(spec, source_id);
        if candidate.join("Makefile").is_file() {
            return Some(candidate);
        }
    }
    for key in ["GAIA_BUILDROOT_DIR", "BUILDROOT_DIR"] {
        if let Some(candidate) = env::var_os(key).map(PathBuf::from)
            && candidate.join("Makefile").is_file()
        {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn buildroot_source_id(image: &ImageSpec) -> Option<&SourceId> {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.source.as_ref(),
        _ => None,
    }
}

pub(crate) fn buildroot_allow_fallback(image: &ImageSpec) -> bool {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.allow_fallback,
        _ => false,
    }
}

pub(crate) fn buildroot_source_dir(spec: &ResolvedBuildSpec, source_id: &SourceId) -> PathBuf {
    Path::new(&spec.workspace.root_dir)
        .join(&spec.workspace.build_dir)
        .join("sources")
        .join(source_id.as_str())
}

#[derive(Clone)]
pub(crate) struct ImageCommandContext<'a> {
    pub(crate) execution: &'a ImageExecutionContext,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

pub(crate) struct BuildrootRunRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) command: ImageCommandContext<'a>,
}

/// Extra behavior for the final `make` of [`run_buildroot_with`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BuildrootMakeOptions<'a> {
    /// Appended to `BR2_ROOTFS_POST_BUILD_SCRIPT` on the make command line
    /// (the `.config` file is left untouched).
    pub(crate) post_build_script: Option<&'a Path>,
    /// The output dir is a shared tree: once its fakeroot scripts match the
    /// current config, only `make target-finalize` runs, because every build
    /// packs its own images.
    pub(crate) shared_tree: bool,
}

pub(crate) fn run_buildroot(
    request: BuildrootRunRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    run_buildroot_with(request, BuildrootMakeOptions::default())
}

pub(crate) fn run_buildroot_with(
    request: BuildrootRunRequest<'_>,
    options: BuildrootMakeOptions<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootRunRequest {
        spec,
        image,
        buildroot_dir,
        output_dir,
        command: command_context,
    } = request;
    fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot output dir '{}': {error}",
            output_dir.display()
        ))
    })?;
    let mut messages = Vec::new();

    let (defconfig, defconfig_path, config_fragments, config_overrides, external_tree) =
        match &image.definition {
            ImageDefinition::Buildroot(buildroot) => (
                buildroot.defconfig.as_deref(),
                buildroot.defconfig_path.as_deref(),
                buildroot.config_fragments.as_slice(),
                buildroot.config_overrides.as_slice(),
                buildroot.external_tree.as_deref(),
            ),
            _ => (None, None, &[][..], &[][..], None),
        };

    let package_overrides =
        materialize_buildroot_package_overrides(spec, buildroot_dir, output_dir)?;
    if package_overrides.generated_external_tree.is_some() {
        ensure_no_generated_external_name_conflict(external_tree)?;
    }
    let br2_external = buildroot_external_tree_value(
        spec,
        external_tree,
        package_overrides
            .generated_external_tree
            .as_ref()
            .map(|generated| generated.path.as_path()),
    );
    let br2_external = br2_external.as_deref();
    if let Some(generated_external_tree) = &package_overrides.generated_external_tree {
        messages.push(format!(
            "staged {} generated Buildroot external package override(s) at '{}'",
            generated_external_tree.package_count,
            generated_external_tree.path.display()
        ));
    }
    if package_overrides.replacement_count > 0 {
        messages.push(format!(
            "replaced {} Buildroot source package definition(s)",
            package_overrides.replacement_count
        ));
    }

    if let Some(defconfig_path) = defconfig_path {
        let resolved_defconfig_path = resolve_workspace_path(
            &ResolvedBuildSpec {
                workspace: spec.workspace.clone(),
                ..spec.clone()
            },
            defconfig_path,
        )?;
        materialize_defconfig_support_files(&resolved_defconfig_path, output_dir)?;
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg("defconfig")
            .arg(format!(
                "BR2_DEFCONFIG={}",
                resolved_defconfig_path.display()
            ))
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2_external {
            command.env("BR2_EXTERNAL", br2_external);
        }
        messages.extend(run_command(
            command,
            "buildroot defconfig",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if !config_fragments.is_empty() {
            messages.extend(apply_buildroot_config_fragments(
                spec,
                buildroot_dir,
                output_dir,
                config_fragments,
                br2_external,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree: br2_external,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
        messages.extend(apply_buildroot_cache_config(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            command_context.clone(),
        )?);
    } else if let Some(defconfig) = defconfig {
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg(defconfig)
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2_external {
            command.env("BR2_EXTERNAL", br2_external);
        }
        messages.extend(run_command(
            command,
            "buildroot defconfig",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if !config_fragments.is_empty() {
            messages.extend(apply_buildroot_config_fragments(
                spec,
                buildroot_dir,
                output_dir,
                config_fragments,
                br2_external,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree: br2_external,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
        messages.extend(apply_buildroot_cache_config(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            command_context.clone(),
        )?);
    } else if !config_fragments.is_empty() || !config_overrides.is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "buildroot config_fragments/config_overrides require defconfig or defconfig_path",
        ));
    }

    let config_digest = buildroot_config_digest(output_dir);
    let accepted_config_digests = [
        buildroot_config_digest_v1(output_dir),
        buildroot_legacy_config_digest(output_dir),
    ];
    let replacement_clean_needed =
        package_overrides
            .replacement_digest
            .as_deref()
            .is_some_and(|replacement_digest| {
                buildroot_state_needs_clean(
                    output_dir,
                    ".gaia-buildroot-package-replacements-state",
                    replacement_digest,
                )
            });
    let config_clean_needed = config_digest.as_deref().is_some_and(|config_digest| {
        buildroot_state_needs_clean(output_dir, ".gaia-buildroot-config-state", config_digest)
            && accepted_config_digests
                .iter()
                .flatten()
                .all(|older_digest| {
                    buildroot_state_needs_clean(
                        output_dir,
                        ".gaia-buildroot-config-state",
                        older_digest,
                    )
                })
    });
    if buildroot_legacy_disabled(config_overrides) {
        disable_buildroot_legacy_flag(output_dir)?;
    }
    // Every config step is done: fail (or warn) about requested overrides
    // that olddefconfig dropped, before the clean and the long make.
    messages.extend(check_buildroot_config_overrides(
        spec,
        output_dir,
        config_overrides,
        command_context.policy.override_check,
    )?);
    if replacement_clean_needed || config_clean_needed {
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg("clean")
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2_external {
            command.env("BR2_EXTERNAL", br2_external);
        }
        messages.extend(run_command(
            command,
            "buildroot clean for package replacements",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if replacement_clean_needed {
            messages.push("cleaned Buildroot output for changed package replacements".into());
        }
        if config_clean_needed {
            messages.push("cleaned Buildroot output for changed effective config".into());
        }
    }

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .current_dir(buildroot_dir);
    append_make_jobs(&mut command, command_context.policy.local_jobs);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(br2_external) = br2_external {
        command.env("BR2_EXTERNAL", br2_external);
    }
    if buildroot_legacy_disabled(config_overrides) {
        disable_buildroot_legacy_flag(output_dir)?;
    }
    // Record the state the output tree now corresponds to before the long
    // make: if the build fails, retrying with the same inputs must resume it
    // rather than clean everything again.
    if let Some(replacement_digest) = package_overrides.replacement_digest.as_deref() {
        write_buildroot_state(
            output_dir,
            ".gaia-buildroot-package-replacements-state",
            replacement_digest,
        )?;
    }
    if let Some(config_digest) = config_digest.as_deref() {
        write_buildroot_state(output_dir, ".gaia-buildroot-config-state", config_digest)?;
    }
    if let Some(script) = options.post_build_script {
        command.arg(post_build_script_override(output_dir, script));
    }
    let mut finalize_only = false;
    if options.shared_tree {
        let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
        let fs_types = shared_rootfs_types(&config)?;
        finalize_only = shared_pack_scripts_current(output_dir, &fs_types);
        if finalize_only {
            command.arg("target-finalize");
        } else {
            clear_shared_pack_state(output_dir)?;
        }
    }
    messages.extend(run_command(
        command,
        "buildroot make",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )?);
    if options.shared_tree && !finalize_only {
        write_shared_pack_state(output_dir)?;
    }
    Ok(messages)
}

/// Settings that only say where to download or cache things, or how many
/// jobs to run. Changing them cannot change what is built, so they must not
/// trigger a full clean.
const BUILDROOT_NON_OUTPUT_SETTINGS: &[&str] = &[
    "BR2_DL_DIR",
    "BR2_CCACHE_DIR",
    "BR2_CCACHE_INITIAL_SETUP",
    "BR2_JLEVEL",
    "BR2_PRIMARY_SITE",
    "BR2_BACKUP_SITE",
    "BR2_KERNEL_MIRROR",
    "BR2_GNU_MIRROR",
    "BR2_LUAROCKS_MIRROR",
    "BR2_CPAN_MIRROR",
];

/// Squashfs tuning (compression, block size, padding). Root filesystem
/// images are regenerated from `target/` by every `make`, and the host
/// squashfs tools support every compressor, so switching XZ for zstd in a
/// development preset needs no clean.
fn is_squashfs_tuning_setting(key: &str) -> bool {
    key.starts_with("BR2_TARGET_ROOTFS_SQUASHFS") && key != "BR2_TARGET_ROOTFS_SQUASHFS"
}

/// Digest of the effective `.config` settings. Excluded: the generated header,
/// which names the Buildroot version (with a `-g<sha>` suffix for git trees),
/// the settings in [`BUILDROOT_NON_OUTPUT_SETTINGS`] and squashfs tuning.
/// "is not set" lines are kept.
pub(crate) fn buildroot_config_digest(output_dir: &Path) -> Option<String> {
    buildroot_settings_digest(output_dir, true).map(|hex| format!("settings-v2-sha256:{hex}"))
}

/// The digest written before squashfs tuning was excluded; still accepted so
/// an upgrade does not force a full Buildroot clean.
fn buildroot_config_digest_v1(output_dir: &Path) -> Option<String> {
    buildroot_settings_digest(output_dir, false).map(|hex| format!("settings-sha256:{hex}"))
}

fn buildroot_settings_digest(output_dir: &Path, skip_squashfs_tuning: bool) -> Option<String> {
    let contents = fs::read_to_string(output_dir.join(".config")).ok()?;
    let settings = contents
        .lines()
        .filter(|line| !is_buildroot_config_header(line))
        .filter(|line| {
            let key = line
                .trim_start_matches("# ")
                .split(['=', ' '])
                .next()
                .unwrap_or_default();
            let squashfs_tuning = skip_squashfs_tuning && is_squashfs_tuning_setting(key);
            !(BUILDROOT_NON_OUTPUT_SETTINGS.contains(&key) || squashfs_tuning)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut hasher = Sha256::new();
    hasher.update(settings.as_bytes());
    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

/// Whole-file digest written by earlier Gaia versions; still accepted so an
/// upgrade does not force a full Buildroot clean.
fn buildroot_legacy_config_digest(output_dir: &Path) -> Option<String> {
    let config_path = output_dir.join(".config");
    config_path
        .is_file()
        .then(|| file_sha256_or_placeholder(&config_path))
}

fn is_buildroot_config_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed == "#"
        || trimmed.starts_with("# Automatically generated file")
        || (trimmed.starts_with("# Buildroot ") && trimmed.ends_with(" Configuration"))
}

fn buildroot_state_needs_clean(output_dir: &Path, state_file: &str, digest: &str) -> bool {
    let state_path = output_dir.join(state_file);
    match fs::read_to_string(state_path) {
        Ok(state) => state.trim() != digest,
        Err(_) => buildroot_output_has_prior_build(output_dir),
    }
}

pub(crate) fn buildroot_legacy_disabled(overrides: &[(String, String)]) -> bool {
    overrides
        .iter()
        .any(|(key, value)| key == "BR2_LEGACY" && value.trim() == "n")
}

pub(crate) fn disable_buildroot_legacy_flag(output_dir: &Path) -> Result<(), ImageProviderError> {
    let config_path = output_dir.join(".config");
    if !config_path.is_file() {
        return Ok(());
    }
    let config = fs::read_to_string(&config_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read Buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;
    if !config.lines().any(|line| line.trim() == "BR2_LEGACY=y") {
        return Ok(());
    }
    let rewritten = config
        .lines()
        .map(|line| {
            if line.trim() == "BR2_LEGACY=y" {
                "# BR2_LEGACY is not set"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&config_path, rewritten).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write Buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })
}

fn buildroot_output_has_prior_build(output_dir: &Path) -> bool {
    ["build", "target", "images"]
        .iter()
        .any(|entry| output_dir.join(entry).exists())
}

fn write_buildroot_state(
    output_dir: &Path,
    state_file: &str,
    digest: &str,
) -> Result<(), ImageProviderError> {
    fs::write(output_dir.join(state_file), format!("{digest}\n")).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write Buildroot state '{}' in '{}': {error}",
            state_file,
            output_dir.display()
        ))
    })
}

pub(crate) fn buildroot_state_digest(image: &ImageSpec, output_dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    output_dir
        .join(".config")
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&output_dir.join(".config")).hash(&mut hasher);
    image_feed_signature_path(output_dir)
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&image_feed_signature_path(output_dir)).hash(&mut hasher);
    if let ImageDefinition::Buildroot(buildroot) = &image.definition {
        for expected in &buildroot.expected_images {
            expected.name.hash(&mut hasher);
            expected.format.as_str().hash(&mut hasher);
            expected.required.hash(&mut hasher);
            file_state_for_digest(&output_dir.join("images").join(&expected.name))
                .hash(&mut hasher);
            file_state_for_digest(&output_dir.join(&expected.name)).hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

fn file_state_for_digest(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => format!(
            "file:{}:{}",
            metadata.len(),
            file_sha256_or_placeholder(path)
        ),
        Ok(metadata) if metadata.is_dir() => format!("dir:{}", metadata.len()),
        Ok(_) => "other".to_string(),
        Err(_) => "missing".to_string(),
    }
}
