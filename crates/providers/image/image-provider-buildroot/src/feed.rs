use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn collect_expected_images(
    image: &ImageSpec,
    output_dir: &Path,
    collect_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return Ok(Vec::new());
    };
    let images_dir = output_dir.join("images");
    let assembly_expected = assembly_expected_image_names(image);
    let mut collected = std::collections::BTreeSet::new();
    let mut matched = Vec::new();
    for expected in &buildroot.expected_images {
        if assembly_expected.contains(&expected.name) {
            continue;
        }
        let candidates = [
            images_dir.join(&expected.name),
            output_dir.join(&expected.name),
        ];
        let found = candidates.iter().find(|path| path.exists()).cloned();
        match found {
            Some(found_path) => {
                fs::create_dir_all(collect_dir).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to create buildroot collect dir '{}': {error}",
                            collect_dir.display()
                        ),
                    )
                })?;
                let dest = collect_dir.join(&expected.name);
                fs::copy(&found_path, &dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to copy expected buildroot image '{}' to '{}': {error}",
                            found_path.display(),
                            dest.display()
                        ),
                    )
                })?;
                collected.insert(PathBuf::from(&expected.name));
                matched.push(expected.name.clone());
            }
            None if expected.required => {
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::OutputMissing,
                    format!(
                        "required buildroot expected image '{}' was not produced",
                        expected.name
                    ),
                ));
            }
            None => {}
        }
    }
    for input in assembly_provider_image_inputs(image) {
        if !collected.insert(input.clone()) {
            continue;
        }
        let candidates = [images_dir.join(&input), output_dir.join(&input)];
        let Some(found_path) = candidates.iter().find(|path| path.exists()).cloned() else {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                format!(
                    "required buildroot assembly input '{}' was not produced",
                    input.display()
                ),
            ));
        };
        let dest = collect_dir.join(&input);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to create buildroot assembly input dir '{}': {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        fs::copy(&found_path, &dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy buildroot assembly input '{}' to '{}': {error}",
                    found_path.display(),
                    dest.display()
                ),
            )
        })?;
    }
    Ok(matched)
}

pub(crate) fn buildroot_expected_images_present(image: &ImageSpec, output_dir: &Path) -> bool {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return false;
    };
    if buildroot.expected_images.is_empty() {
        return false;
    }
    let assembly_expected = assembly_expected_image_names(image);
    let concrete_required = buildroot
        .expected_images
        .iter()
        .filter(|expected| expected.required)
        .filter(|expected| !assembly_expected.contains(&expected.name))
        .collect::<Vec<_>>();
    let provider_image_inputs = assembly_provider_image_inputs(image);
    if concrete_required.is_empty() && provider_image_inputs.is_empty() {
        return false;
    }
    concrete_required.iter().all(|expected| {
        output_dir.join("images").join(&expected.name).is_file()
            || output_dir.join(&expected.name).is_file()
    }) && provider_image_inputs.iter().all(|input| {
        output_dir
            .parent()
            .is_some_and(|collect_dir| collect_dir.join(input).is_file())
            || output_dir.join("images").join(input).is_file()
            || output_dir.join(input).is_file()
    })
}

pub(crate) fn materialize_fallback_rootfs(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    rootfs_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let execution = execution_context(spec);
    if rootfs_dir.exists() {
        fs::remove_dir_all(rootfs_dir).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to clean fallback rootfs dir '{}': {error}",
                    rootfs_dir.display()
                ),
            )
        })?;
    }
    fs::create_dir_all(rootfs_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create fallback rootfs dir '{}': {error}",
                rootfs_dir.display()
            ),
        )
    })?;

    apply_image_feed_to_rootfs(spec, image, rootfs_dir)?;

    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return Ok(Vec::new());
    };
    let assembly_expected = assembly_expected_image_names(image);
    let mut matched = Vec::new();
    for expected in &buildroot.expected_images {
        if assembly_expected.contains(&expected.name) {
            continue;
        }
        if expected.format == BuildrootExpectedImageFormatSpec::Tar {
            let tar_path = rootfs_dir
                .parent()
                .unwrap_or(rootfs_dir)
                .join(&expected.name);
            archive_directory(
                rootfs_dir,
                &tar_path,
                "buildroot expected image",
                &execution,
                &ImageExecutionPolicy::default(),
                None,
                None,
            )?;
            matched.push(expected.name.clone());
        } else if expected.required {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                format!(
                    "fallback buildroot mode cannot produce required non-tar image '{}'",
                    expected.name
                ),
            ));
        }
    }
    Ok(matched)
}

fn assembly_expected_image_names(image: &ImageSpec) -> std::collections::HashSet<String> {
    let Some(assembly) = &image.assembly else {
        return std::collections::HashSet::new();
    };
    assembly
        .filesystems
        .iter()
        .map(|filesystem| filesystem.output.as_str())
        .chain(assembly.disks.iter().map(|disk| disk.output.as_str()))
        .filter_map(|output| {
            Path::new(output)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect()
}

fn assembly_provider_image_inputs(image: &ImageSpec) -> std::collections::BTreeSet<PathBuf> {
    let Some(assembly) = &image.assembly else {
        return std::collections::BTreeSet::new();
    };
    let generated_outputs = assembly
        .filesystems
        .iter()
        .map(|filesystem| filesystem.output.as_str())
        .chain(assembly.disks.iter().map(|disk| disk.output.as_str()))
        .collect::<std::collections::HashSet<_>>();
    assembly
        .disks
        .iter()
        .flat_map(|disk| &disk.partitions)
        .filter_map(|partition| {
            let image = partition.image.as_str();
            if generated_outputs.contains(image) {
                return None;
            }
            image
                .strip_prefix("$provider.images/")
                .filter(|relative| !relative.trim().is_empty())
                .map(PathBuf::from)
        })
        .collect()
}

pub(crate) fn apply_image_feed_to_rootfs(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    rootfs_dir: &Path,
) -> Result<(), ImageProviderError> {
    for install_id in &image.feed.install_entries {
        let install = spec
            .install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown install '{}'",
                        install_id.as_str()
                    ),
                )
            })?;
        let artifact = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == install.artifact.id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "install '{}' references unknown artifact '{}'",
                        install.id.as_str(),
                        install.artifact.id.as_str()
                    ),
                )
            })?;
        let src = if Path::new(&artifact.output.path).is_absolute() {
            PathBuf::from(&artifact.output.path)
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(&artifact.output.path)
        };
        if !src.exists() {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                format!(
                    "install artifact output missing for '{}': {}",
                    artifact.id.as_str(),
                    src.display()
                ),
            ));
        }
        verify_install_artifact_target(artifact, &src)?;
        let dest = rootfs_path(rootfs_dir, &install.dest);
        copy_path(&src, &dest)?;
        #[cfg(unix)]
        if let Some(mode) = install.mode {
            let permissions = fs::Permissions::from_mode(mode);
            fs::set_permissions(&dest, permissions).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to set install mode on '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }

    for stage_file_id in &image.feed.stage_files {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage file '{}'",
                        stage_file_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &stage_file.src)?;
        let dest = rootfs_path(rootfs_dir, &stage_file.dest);
        copy_path(&src, &dest)?;
        #[cfg(unix)]
        if let Some(mode) = stage_file.mode {
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode)).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to set stage file mode on '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }

    for env_set_id in &image.feed.stage_env_sets {
        let env_set = spec
            .stage
            .env_sets
            .iter()
            .find(|env_set| env_set.id == *env_set_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage env set '{}'",
                        env_set_id.as_str()
                    ),
                )
            })?;
        let dest = rootfs_dir
            .join("etc")
            .join("default")
            .join(format!("{}.env", env_set.name));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to create env set dir '{}': {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        let mut contents = String::new();
        for (key, value) in &env_set.entries {
            contents.push_str(key);
            contents.push('=');
            contents.push_str(value);
            contents.push('\n');
        }
        fs::write(&dest, contents).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to write env set '{}': {error}", dest.display()),
            )
        })?;
    }

    for service_id in &image.feed.stage_services {
        let service = spec
            .stage
            .services
            .iter()
            .find(|service| service.id == *service_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage service '{}'",
                        service_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &service.unit_path)?;
        let dest = rootfs_dir
            .join("etc")
            .join("systemd")
            .join("system")
            .join(&service.name);
        copy_path(&src, &dest)?;
    }
    Ok(())
}

pub(crate) fn verify_install_artifact_target(
    artifact: &gaia_spec::ArtifactSpec,
    src: &Path,
) -> Result<(), ImageProviderError> {
    let Some(target) = artifact.target.as_deref() else {
        return Ok(());
    };
    if target.trim().is_empty() || src.is_dir() {
        return Ok(());
    }

    let expected_machine = expected_elf_machine_for_target(target).ok_or_else(|| {
        ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "artifact '{}' declares target '{}', but Gaia cannot verify that target against the installed output yet",
                artifact.id.as_str(),
                target
            ),
        )
    })?;
    let actual_machine = detect_elf_machine(src).ok_or_else(|| {
        ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "artifact '{}' declares target '{}', but installed output '{}' is not a verifiable ELF binary",
                artifact.id.as_str(),
                target,
                src.display()
            ),
        )
    })?;
    if actual_machine != expected_machine {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "artifact '{}' target mismatch: declared '{}' but installed output '{}' is '{}'",
                artifact.id.as_str(),
                target,
                src.display(),
                actual_machine.label()
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElfMachine {
    X86_64,
    Arm,
    AArch64,
    RiscV64,
}

impl ElfMachine {
    fn label(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm => "arm",
            Self::AArch64 => "aarch64",
            Self::RiscV64 => "riscv64",
        }
    }
}

pub(crate) fn expected_elf_machine_for_target(target: &str) -> Option<ElfMachine> {
    let lowered = target.trim().to_ascii_lowercase();
    if lowered == "aarch64-unknown-linux-gnu" || lowered == "linux/arm64" {
        return Some(ElfMachine::AArch64);
    }
    if lowered == "x86_64-unknown-linux-gnu" || lowered == "linux/amd64" {
        return Some(ElfMachine::X86_64);
    }
    if lowered == "riscv64gc-unknown-linux-gnu" || lowered == "linux/riscv64" {
        return Some(ElfMachine::RiscV64);
    }
    if lowered == "linux/arm"
        || lowered.starts_with("linux/arm/")
        || lowered.starts_with("arm-unknown-linux-")
        || lowered.starts_with("armv7-unknown-linux-")
        || lowered.starts_with("armv6-unknown-linux-")
    {
        return Some(ElfMachine::Arm);
    }
    None
}

pub(crate) fn detect_elf_machine(path: &Path) -> Option<ElfMachine> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 20 || &bytes[0..4] != b"\x7FELF" {
        return None;
    }
    let little_endian = match bytes[5] {
        1 => true,
        2 => false,
        _ => return None,
    };
    let e_machine = if little_endian {
        u16::from_le_bytes([bytes[18], bytes[19]])
    } else {
        u16::from_be_bytes([bytes[18], bytes[19]])
    };
    match e_machine {
        0x3E => Some(ElfMachine::X86_64),
        0x28 => Some(ElfMachine::Arm),
        0xB7 => Some(ElfMachine::AArch64),
        0xF3 => Some(ElfMachine::RiscV64),
        _ => None,
    }
}

pub(crate) fn refresh_expected_tar_images(
    image: &ImageSpec,
    rootfs_dir: &Path,
    output_dir: &Path,
    execution: &ImageExecutionContext,
) -> Result<(), ImageProviderError> {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return Ok(());
    };
    let images_dir = output_dir.join("images");
    fs::create_dir_all(&images_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create buildroot images dir '{}': {error}",
                images_dir.display()
            ),
        )
    })?;
    for expected in &buildroot.expected_images {
        if expected.format == BuildrootExpectedImageFormatSpec::Tar {
            archive_directory(
                rootfs_dir,
                &images_dir.join(&expected.name),
                "buildroot expected image",
                execution,
                &ImageExecutionPolicy::default(),
                None,
                None,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn image_feed_has_content(image: &ImageSpec) -> bool {
    !image.feed.install_entries.is_empty()
        || !image.feed.stage_files.is_empty()
        || !image.feed.stage_env_sets.is_empty()
        || !image.feed.stage_services.is_empty()
}

pub(crate) fn image_feed_signature_path(output_dir: &Path) -> PathBuf {
    output_dir.join(".gaia-image-feed-state.txt")
}

pub(crate) fn image_feed_managed_paths_path(output_dir: &Path) -> PathBuf {
    output_dir.join(".gaia-image-feed-managed-paths.txt")
}

pub(crate) fn image_feed_signature_is_current(output_dir: &Path, signature: &str) -> bool {
    fs::read_to_string(image_feed_signature_path(output_dir))
        .is_ok_and(|current| current == signature)
}

pub(crate) fn image_feed_outputs_present(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    rootfs_dir: &Path,
) -> bool {
    image.feed.install_entries.iter().all(|install_id| {
        spec.install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)
            .map(|install| rootfs_path(rootfs_dir, &install.dest).exists())
            .unwrap_or(false)
    }) && image.feed.stage_files.iter().all(|stage_file_id| {
        spec.stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .map(|stage_file| rootfs_path(rootfs_dir, &stage_file.dest).exists())
            .unwrap_or(false)
    }) && image.feed.stage_env_sets.iter().all(|env_set_id| {
        spec.stage
            .env_sets
            .iter()
            .find(|env_set| env_set.id == *env_set_id)
            .map(|env_set| {
                rootfs_dir
                    .join("etc")
                    .join("default")
                    .join(format!("{}.env", env_set.name))
                    .exists()
            })
            .unwrap_or(false)
    }) && image.feed.stage_services.iter().all(|service_id| {
        spec.stage
            .services
            .iter()
            .find(|service| service.id == *service_id)
            .map(|service| {
                rootfs_dir
                    .join("etc")
                    .join("systemd")
                    .join("system")
                    .join(&service.name)
                    .exists()
            })
            .unwrap_or(false)
    })
}

pub(crate) fn prune_stale_image_feed_outputs(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    rootfs_dir: &Path,
    output_dir: &Path,
) -> Result<(), ImageProviderError> {
    let current_paths = image_feed_managed_paths(spec, image)?;
    let mut previous_paths = read_image_feed_managed_paths(output_dir)?;
    previous_paths.extend(stale_runtime_state_managed_paths(spec)?);
    for previous in previous_paths {
        if current_paths.contains(&previous) {
            continue;
        }
        let dest = rootfs_path(rootfs_dir, &previous);
        remove_path_if_exists(&dest)?;
        prune_empty_parent_dirs(&dest, rootfs_dir)?;
    }
    Ok(())
}

fn stale_runtime_state_managed_paths(
    spec: &ResolvedBuildSpec,
) -> Result<BTreeSet<String>, ImageProviderError> {
    let runtime_dir =
        PathBuf::from(&spec.workspace.out_dir).join(gaia_spec::RUNTIME_STATE_DIR_NAME);
    let entries = match fs::read_dir(&runtime_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read runtime state dir '{}': {error}",
                    runtime_dir.display()
                ),
            ));
        }
    };

    let current_install_ids = spec
        .image
        .feed
        .install_entries
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    let current_stage_file_ids = spec
        .image
        .feed
        .stage_files
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    let current_stage_env_ids = spec
        .image
        .feed
        .stage_env_sets
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    let current_stage_service_ids = spec
        .image
        .feed
        .stage_services
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();

    let mut paths = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read runtime state entry in '{}': {error}",
                    runtime_dir.display()
                ),
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("state") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let contents = fs::read_to_string(&path).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read runtime state '{}': {error}", path.display()),
            )
        })?;
        let state = parse_key_value_state(&contents);
        if let Some(id) = file_name
            .strip_prefix("install-")
            .and_then(|name| name.strip_suffix(".state"))
            && !current_install_ids.contains(id)
            && let Some(dest) = state.get("dest")
        {
            paths.insert(dest.clone());
        }
        if let Some(id) = file_name
            .strip_prefix("stage-file-")
            .and_then(|name| name.strip_suffix(".state"))
            && !current_stage_file_ids.contains(id)
            && let Some(dest) = state.get("dest")
        {
            paths.insert(dest.clone());
        }
        if let Some(id) = file_name
            .strip_prefix("stage-env-")
            .and_then(|name| name.strip_suffix(".state"))
            && !current_stage_env_ids.contains(id)
            && let Some(name) = state.get("name")
        {
            paths.insert(format!("/etc/default/{name}.env"));
        }
        if let Some(id) = file_name
            .strip_prefix("stage-service-")
            .and_then(|name| name.strip_suffix(".state"))
            && !current_stage_service_ids.contains(id)
            && let Some(name) = state.get("name")
        {
            paths.insert(format!("/etc/systemd/system/{name}"));
        }
    }
    Ok(paths)
}

fn parse_key_value_state(contents: &str) -> BTreeMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

pub(crate) fn write_image_feed_signature(
    output_dir: &Path,
    signature: &str,
) -> Result<(), ImageProviderError> {
    fs::write(image_feed_signature_path(output_dir), signature).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write image feed state in '{}': {error}",
                output_dir.display()
            ),
        )
    })
}

pub(crate) fn write_image_feed_managed_paths(
    output_dir: &Path,
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Result<(), ImageProviderError> {
    let mut body = String::from("gaia-image-feed-managed-paths-v1\n");
    for path in image_feed_managed_paths(spec, image)? {
        body.push_str(&path);
        body.push('\n');
    }
    fs::write(image_feed_managed_paths_path(output_dir), body).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write image feed managed paths '{}': {error}",
                image_feed_managed_paths_path(output_dir).display()
            ),
        )
    })
}

fn read_image_feed_managed_paths(
    output_dir: &Path,
) -> Result<std::collections::BTreeSet<String>, ImageProviderError> {
    let path = image_feed_managed_paths_path(output_dir);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(std::collections::BTreeSet::new());
        }
        Err(error) => {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read image feed managed paths '{}': {error}",
                    path.display()
                ),
            ));
        }
    };
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "gaia-image-feed-managed-paths-v1")
        .map(str::to_string)
        .collect())
}

fn image_feed_managed_paths(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Result<std::collections::BTreeSet<String>, ImageProviderError> {
    let mut paths = std::collections::BTreeSet::new();

    for install_id in &image.feed.install_entries {
        let install = spec
            .install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown install '{}'",
                        install_id.as_str()
                    ),
                )
            })?;
        paths.insert(install.dest.clone());
    }

    for stage_file_id in &image.feed.stage_files {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage file '{}'",
                        stage_file_id.as_str()
                    ),
                )
            })?;
        paths.insert(stage_file.dest.clone());
    }

    for env_set_id in &image.feed.stage_env_sets {
        let env_set = spec
            .stage
            .env_sets
            .iter()
            .find(|env_set| env_set.id == *env_set_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage env set '{}'",
                        env_set_id.as_str()
                    ),
                )
            })?;
        paths.insert(format!("/etc/default/{}.env", env_set.name));
    }

    for service_id in &image.feed.stage_services {
        let service = spec
            .stage
            .services
            .iter()
            .find(|service| service.id == *service_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage service '{}'",
                        service_id.as_str()
                    ),
                )
            })?;
        paths.insert(format!("/etc/systemd/system/{}", service.name));
    }

    Ok(paths)
}

fn remove_path_if_exists(path: &Path) -> Result<(), ImageProviderError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to remove stale image feed directory '{}': {error}",
                    path.display()
                ),
            )
        }),
        Ok(_) => fs::remove_file(path).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to remove stale image feed file '{}': {error}",
                    path.display()
                ),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to inspect stale image feed path '{}': {error}",
                path.display()
            ),
        )),
    }
}

fn prune_empty_parent_dirs(path: &Path, stop_at: &Path) -> Result<(), ImageProviderError> {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == stop_at {
            break;
        }
        match fs::remove_dir(dir) {
            Ok(()) => current = dir.parent(),
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to prune stale image feed parent '{}': {error}",
                        dir.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn build_image_feed_signature(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Result<String, ImageProviderError> {
    let mut signature = String::from("gaia-image-feed-v1\n");
    signature.push_str("installs:\n");
    for install_id in &image.feed.install_entries {
        let install = spec
            .install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown install '{}'",
                        install_id.as_str()
                    ),
                )
            })?;
        let artifact = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == install.artifact.id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "install '{}' references unknown artifact '{}'",
                        install.id.as_str(),
                        install.artifact.id.as_str()
                    ),
                )
            })?;
        let src = if Path::new(&artifact.output.path).is_absolute() {
            PathBuf::from(&artifact.output.path)
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(&artifact.output.path)
        };
        signature.push_str(&format!(
            "{}|{}|{}|{:?}|{}\n",
            install.id.as_str(),
            artifact.id.as_str(),
            install.dest,
            install.mode,
            dir_digest(&src)
        ));
    }

    signature.push_str("stage-files:\n");
    for stage_file_id in &image.feed.stage_files {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage file '{}'",
                        stage_file_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &stage_file.src)?;
        signature.push_str(&format!(
            "{}|{}|{:?}|{}\n",
            stage_file.id.as_str(),
            stage_file.dest,
            stage_file.mode,
            dir_digest(&src)
        ));
    }

    signature.push_str("env-sets:\n");
    for env_set_id in &image.feed.stage_env_sets {
        let env_set = spec
            .stage
            .env_sets
            .iter()
            .find(|env_set| env_set.id == *env_set_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage env set '{}'",
                        env_set_id.as_str()
                    ),
                )
            })?;
        signature.push_str(&format!("{}|{}\n", env_set.id.as_str(), env_set.name));
        for (key, value) in &env_set.entries {
            signature.push_str(&format!("{key}={value}\n"));
        }
    }

    signature.push_str("services:\n");
    for service_id in &image.feed.stage_services {
        let service = spec
            .stage
            .services
            .iter()
            .find(|service| service.id == *service_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage service '{}'",
                        service_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &service.unit_path)?;
        signature.push_str(&format!(
            "{}|{}|{}\n",
            service.id.as_str(),
            service.name,
            dir_digest(&src)
        ));
    }

    Ok(signature)
}
