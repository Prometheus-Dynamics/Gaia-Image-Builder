use super::*;

pub(crate) fn collect_expected_images(
    image: &ImageSpec,
    output_dir: &Path,
    collect_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return Ok(Vec::new());
    };
    let images_dir = output_dir.join("images");
    let mut matched = Vec::new();
    for expected in &buildroot.expected_images {
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
    Ok(matched)
}

pub(crate) fn buildroot_expected_images_present(image: &ImageSpec, output_dir: &Path) -> bool {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return false;
    };
    if buildroot.expected_images.is_empty() {
        return false;
    }
    buildroot
        .expected_images
        .iter()
        .filter(|expected| expected.required)
        .all(|expected| {
            output_dir.join("images").join(&expected.name).is_file()
                || output_dir.join(&expected.name).is_file()
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
    let mut matched = Vec::new();
    for expected in &buildroot.expected_images {
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

pub(crate) fn image_feed_signature_is_current(output_dir: &Path, signature: &str) -> bool {
    fs::read_to_string(image_feed_signature_path(output_dir))
        .is_ok_and(|current| current == signature)
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
            "{}|{}|{}\n",
            stage_file.id.as_str(),
            stage_file.dest,
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
