use super::*;

pub(crate) fn starting_point_spec(
    image: &ImageSpec,
) -> Result<&gaia_spec::StartingPointImageSpec, ImageProviderError> {
    match &image.definition {
        ImageDefinition::StartingPoint(starting_point) => Ok(starting_point),
        _ => Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            "expected starting-point image definition".to_string(),
        )),
    }
}

pub(crate) fn starting_point_packages(image: &ImageSpec) -> &StartingPointPackagesSpec {
    match &image.definition {
        ImageDefinition::StartingPoint(starting_point) => &starting_point.packages,
        _ => unreachable!("starting-point packages only valid for starting-point image"),
    }
}

#[cfg(test)]
pub(crate) fn reconcile_packages(
    _rootfs_source: &Path,
    rootfs_dir: &Path,
    packages: &StartingPointPackagesSpec,
) -> Result<Vec<String>, ImageProviderError> {
    Ok(plan_package_reconcile(rootfs_dir, packages)?.messages)
}

pub(crate) fn reconcile_packages_in_rootfs(
    rootfs_dir: &Path,
    packages: &StartingPointPackagesSpec,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    let plan = plan_package_reconcile(rootfs_dir, packages)?;
    if !packages.enabled || !packages.execute || plan.commands.is_empty() {
        return Ok(plan.messages);
    }
    ensure_linux_root("starting-point package reconciliation execute=true requires root")?;
    let runtime_guard =
        prepare_chroot_runtime(rootfs_dir, policy, log_sink.clone(), cancel_check.clone())?;
    let result = run_package_commands_in_chroot(
        rootfs_dir,
        &plan.commands,
        policy,
        log_sink.clone(),
        cancel_check,
    );
    combine_primary_and_cleanup(result, runtime_guard.cleanup())?;
    Ok(plan.messages)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PackageReconcilePlan {
    pub(crate) messages: Vec<String>,
    pub(crate) commands: Vec<PackageCommand>,
}

pub(crate) fn plan_package_reconcile(
    rootfs_dir: &Path,
    packages: &StartingPointPackagesSpec,
) -> Result<PackageReconcilePlan, ImageProviderError> {
    let mut plan = PackageReconcilePlan::default();
    if !packages.enabled {
        return Ok(plan);
    }
    let os_release = read_os_release(rootfs_dir, packages.os_release_path.as_deref())?;
    let version_id = os_release.get("VERSION_ID").cloned();
    validate_package_release_override(version_id.as_deref(), packages)?;
    let manager =
        detect_package_manager(rootfs_dir, packages.manager.as_deref()).ok_or_else(|| {
            ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                "failed to detect package manager for starting-point rootfs".to_string(),
            )
        })?;
    let selected_release_version = packages
        .release_version
        .as_deref()
        .or(version_id.as_deref());
    let commands = build_package_reconcile_commands(&manager, packages, selected_release_version);
    for command in &commands {
        plan.messages.push(format!(
            "starting-point package command: {}",
            command.display()
        ));
    }
    if !packages.execute && !commands.is_empty() {
        plan.messages.push(
            "starting-point package reconcile execute=false; commands were planned only".into(),
        );
    }
    plan.commands = commands;
    Ok(plan)
}

pub(crate) fn validate_package_release_override(
    version_id: Option<&str>,
    packages: &StartingPointPackagesSpec,
) -> Result<(), ImageProviderError> {
    if let (Some(detected), Some(overridden)) = (version_id, packages.release_version.as_deref()) {
        let detected_major = major_version_component(detected);
        let override_major = major_version_component(overridden);
        if detected_major.is_some()
            && override_major.is_some()
            && detected_major != override_major
            && !packages.allow_major_upgrade
        {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                format!(
                    "starting-point package release override '{}' changes major version from '{}' but allow_major_upgrade=false",
                    overridden, detected
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) fn run_package_commands_in_chroot(
    rootfs_dir: &Path,
    commands: &[PackageCommand],
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    for package_command in commands {
        let mut cmd = Command::new("chroot");
        cmd.arg(rootfs_dir)
            .arg(&package_command.program)
            .args(&package_command.args);
        command_status(
            &mut cmd,
            &format!(
                "starting-point package reconcile chroot command '{}'",
                package_command.display()
            ),
            ImageProviderErrorKind::BackendCommand,
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
    }
    Ok(())
}

pub(crate) fn ensure_linux_root(message: &str) -> Result<(), ImageProviderError> {
    #[cfg(target_os = "linux")]
    {
        if unsafe { libc::geteuid() } != 0 {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                message.to_string(),
            ));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = message;
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "starting-point privileged image mutation is only supported on Linux hosts".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImagePartitionInfo {
    pub(crate) path: String,
    pub(crate) fstype: String,
    pub(crate) size_bytes: u64,
}

pub(crate) fn losetup_attach(
    image_path: &Path,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<String, ImageProviderError> {
    let mut attach = Command::new("losetup");
    attach
        .arg("--find")
        .arg("--show")
        .arg("--partscan")
        .arg(image_path);
    let output = command_output(
        &mut attach,
        "starting-point raw image loop attach",
        policy,
        log_sink.clone(),
        cancel_check.clone(),
    )?;
    if !output.status.success() {
        let msg = stderr_or_stdout(&output);
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::BackendCommand,
            format!("losetup failed for '{}': {msg}", image_path.display()),
        ));
    }
    let device = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if device.is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "losetup returned empty loop device for '{}'",
                image_path.display()
            ),
        ));
    }
    let mut partx = Command::new("partx");
    partx.arg("-u").arg(&device);
    let _ = command_status(
        &mut partx,
        "starting-point raw image partition refresh",
        ImageProviderErrorKind::RuntimeState,
        policy,
        log_sink,
        cancel_check,
    );
    Ok(device)
}

pub(crate) fn list_image_partitions(
    loop_device: &str,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<ImagePartitionInfo>, ImageProviderError> {
    let mut lsblk = Command::new("lsblk");
    lsblk
        .arg("-lnbo")
        .arg("NAME,FSTYPE,SIZE,TYPE")
        .arg(loop_device);
    let output = command_output(
        &mut lsblk,
        "starting-point raw image partition list",
        policy,
        log_sink,
        cancel_check,
    )?;
    if !output.status.success() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::BackendCommand,
            format!(
                "lsblk failed for '{loop_device}': {}",
                stderr_or_stdout(&output)
            ),
        ));
    }
    let mut partitions = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let fstype = parts.next().unwrap_or_default().to_string();
        let size_bytes = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let kind = parts.next().unwrap_or_default();
        if kind == "part" {
            partitions.push(ImagePartitionInfo {
                path: name.to_string(),
                fstype,
                size_bytes,
            });
        }
    }
    Ok(partitions)
}

pub(crate) fn choose_image_partition(
    partitions: &[ImagePartitionInfo],
    requested: Option<&str>,
) -> Result<ImagePartitionInfo, ImageProviderError> {
    if partitions.is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::OutputMissing,
            "no partitions detected in raw image".to_string(),
        ));
    }
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(found) = partitions.iter().find(|partition| {
            partition.path == requested
                || partition.path.ends_with(requested)
                || partition
                    .path
                    .trim_start_matches("/dev/")
                    .ends_with(requested)
        }) {
            return Ok(found.clone());
        }
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::OutputMissing,
            format!("requested starting-point image partition '{requested}' not found"),
        ));
    }
    partitions
        .iter()
        .filter(|partition| !partition.fstype.eq_ignore_ascii_case("vfat"))
        .max_by_key(|partition| partition.size_bytes)
        .cloned()
        .or_else(|| {
            partitions
                .iter()
                .max_by_key(|partition| partition.size_bytes)
                .cloned()
        })
        .ok_or_else(|| {
            ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                "no usable partition found in raw image".to_string(),
            )
        })
}

pub(crate) fn read_os_release(
    rootfs_dir: &Path,
    rel: Option<&str>,
) -> Result<BTreeMap<String, String>, ImageProviderError> {
    let raw_rel = rel
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/etc/os-release");
    let path = rootfs_dir.join(raw_rel.trim_start_matches('/'));
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(&path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to read '{}': {error}", path.display()),
        )
    })?;
    let mut out = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            value = value[1..value.len() - 1].to_string();
        }
        out.insert(key.trim().to_string(), value);
    }
    Ok(out)
}

pub(crate) fn detect_package_manager(rootfs_dir: &Path, requested: Option<&str>) -> Option<String> {
    let normalize = |value: &str| value.trim().to_ascii_lowercase();
    if let Some(raw) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        let normalized = normalize(raw);
        if normalized != "auto" {
            return Some(normalized);
        }
    }
    let exists = |rel: &str| rootfs_dir.join(rel.trim_start_matches('/')).is_file();
    if exists("/usr/bin/apt-get") || exists("/bin/apt-get") {
        return Some("apt".into());
    }
    if exists("/usr/bin/dnf") {
        return Some("dnf".into());
    }
    if exists("/usr/bin/yum") {
        return Some("yum".into());
    }
    if exists("/sbin/apk") || exists("/usr/bin/apk") {
        return Some("apk".into());
    }
    if exists("/usr/bin/pacman") {
        return Some("pacman".into());
    }
    if exists("/usr/bin/zypper") {
        return Some("zypper".into());
    }
    None
}

pub(crate) fn build_package_reconcile_commands(
    manager: &str,
    packages: &StartingPointPackagesSpec,
    release_version: Option<&str>,
) -> Vec<PackageCommand> {
    let install = packages
        .install
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let remove = packages
        .remove
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let extra = packages
        .extra_args
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let apt_release_flag = release_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("APT::Default-Release={value}"));
    let mut out = Vec::new();
    match manager {
        "apt" => {
            if packages.update {
                out.push(PackageCommand::new("apt-get", ["update"]));
            }
            if !install.is_empty() {
                let mut args = Vec::new();
                if let Some(flag) = apt_release_flag.clone() {
                    args.push("-o".into());
                    args.push(flag);
                }
                args.push("install".into());
                args.push("-y".into());
                args.extend(extra.clone());
                args.extend(install.clone());
                out.push(PackageCommand::with_args("apt-get", args));
            }
            if !remove.is_empty() {
                let mut args = Vec::new();
                if let Some(flag) = apt_release_flag {
                    args.push("-o".into());
                    args.push(flag);
                }
                args.push("remove".into());
                args.push("-y".into());
                args.extend(extra.clone());
                args.extend(remove.clone());
                out.push(PackageCommand::with_args("apt-get", args));
            }
            if packages.dist_upgrade {
                let mut args = vec!["dist-upgrade".into(), "-y".into()];
                args.extend(extra);
                out.push(PackageCommand::with_args("apt-get", args));
            }
        }
        "apk" => {
            if packages.update {
                out.push(PackageCommand::new("apk", ["update"]));
            }
            if !install.is_empty() {
                let mut args = vec!["add".into()];
                args.extend(extra.clone());
                args.extend(install);
                out.push(PackageCommand::with_args("apk", args));
            }
            if !remove.is_empty() {
                let mut args = vec!["del".into()];
                args.extend(extra);
                args.extend(remove);
                out.push(PackageCommand::with_args("apk", args));
            }
        }
        "dnf" | "yum" => {
            let base = manager.to_string();
            if packages.update {
                out.push(PackageCommand::new(base.clone(), ["makecache"]));
            }
            if !install.is_empty() {
                let mut args = vec!["install".into(), "-y".into()];
                args.extend(extra.clone());
                args.extend(install);
                out.push(PackageCommand::with_args(base.clone(), args));
            }
            if !remove.is_empty() {
                let mut args = vec!["remove".into(), "-y".into()];
                args.extend(extra.clone());
                args.extend(remove);
                out.push(PackageCommand::with_args(base.clone(), args));
            }
            if packages.dist_upgrade {
                let mut args = vec!["upgrade".into(), "-y".into()];
                args.extend(extra);
                out.push(PackageCommand::with_args(base, args));
            }
        }
        "pacman" => {
            if packages.update {
                out.push(PackageCommand::new("pacman", ["-Sy"]));
            }
            if !install.is_empty() {
                let mut args = vec!["-S".into(), "--noconfirm".into()];
                args.extend(extra.clone());
                args.extend(install);
                out.push(PackageCommand::with_args("pacman", args));
            }
            if !remove.is_empty() {
                let mut args = vec!["-R".into(), "--noconfirm".into()];
                args.extend(extra.clone());
                args.extend(remove);
                out.push(PackageCommand::with_args("pacman", args));
            }
            if packages.dist_upgrade {
                let mut args = vec!["-Syu".into(), "--noconfirm".into()];
                args.extend(extra);
                out.push(PackageCommand::with_args("pacman", args));
            }
        }
        "zypper" => {
            if packages.update {
                out.push(PackageCommand::new("zypper", ["refresh"]));
            }
            if !install.is_empty() {
                let mut args = vec!["--non-interactive".into(), "install".into()];
                args.extend(extra.clone());
                args.extend(install);
                out.push(PackageCommand::with_args("zypper", args));
            }
            if !remove.is_empty() {
                let mut args = vec!["--non-interactive".into(), "remove".into()];
                args.extend(extra.clone());
                args.extend(remove);
                out.push(PackageCommand::with_args("zypper", args));
            }
            if packages.dist_upgrade {
                let mut args = vec!["--non-interactive".into(), "update".into()];
                args.extend(extra);
                out.push(PackageCommand::with_args("zypper", args));
            }
        }
        _ => {}
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageCommand {
    program: String,
    args: Vec<String>,
}

impl PackageCommand {
    fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::with_args(program, args.into_iter().map(Into::into).collect())
    }

    fn with_args(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(sh_quote_if_needed)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub(crate) fn sh_quote_if_needed(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '=' | '+'))
    {
        value.to_string()
    } else {
        sh_quote(value)
    }
}

pub(crate) fn sh_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(crate) fn major_version_component(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let major = value
        .split(|c: char| !c.is_ascii_digit())
        .find(|segment| !segment.is_empty())?;
    Some(major.to_string())
}
