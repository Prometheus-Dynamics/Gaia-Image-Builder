use super::*;

pub(crate) fn refresh_buildroot_images_after_feed_overlay(
    image: &ImageSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return Ok(Vec::new());
    };
    let non_tar_expected_images = buildroot
        .expected_images
        .iter()
        .filter(|expected| expected.format != BuildrootExpectedImageFormatSpec::Tar)
        .collect::<Vec<_>>();
    if non_tar_expected_images.is_empty() {
        return Ok(Vec::new());
    }

    if non_tar_expected_images
        .iter()
        .all(|expected| expected.format == BuildrootExpectedImageFormatSpec::Squashfs)
        && let Some(messages) =
            refresh_buildroot_squashfs_images_direct(buildroot_dir, output_dir, execution, policy)?
    {
        return Ok(messages);
    }
    if non_tar_expected_images
        .iter()
        .any(|expected| expected.format == BuildrootExpectedImageFormatSpec::Squashfs)
        && non_tar_expected_images.iter().all(|expected| {
            matches!(
                expected.format,
                BuildrootExpectedImageFormatSpec::Squashfs | BuildrootExpectedImageFormatSpec::Raw
            )
        })
        && let Some(mut messages) =
            refresh_buildroot_squashfs_images_direct(buildroot_dir, output_dir, execution, policy)?
        && let Some(post_image_messages) = refresh_buildroot_post_image_direct(
            buildroot_dir,
            output_dir,
            execution,
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?
    {
        messages.extend(post_image_messages);
        return Ok(messages);
    }

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("target-post-image")
        .current_dir(buildroot_dir);
    if let Some(external_tree) = buildroot.external_tree.as_deref() {
        command.env("BR2_EXTERNAL", external_tree);
    }
    run_command(
        command,
        "buildroot target-post-image refresh",
        execution,
        policy,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn refresh_buildroot_post_image_direct(
    buildroot_dir: &Path,
    output_dir: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Option<Vec<String>>, ImageProviderError> {
    let normalized_output_dir = normalize_path(output_dir);
    let output_dir = normalized_output_dir.as_path();
    let config_path = output_dir.join(".config");
    let config = match fs::read_to_string(&config_path) {
        Ok(config) => config,
        Err(_) => return Ok(None),
    };
    let Some(script_value) = buildroot_config_value(&config, "BR2_ROOTFS_POST_IMAGE_SCRIPT") else {
        return Ok(None);
    };
    if script_value.trim().is_empty() {
        return Ok(None);
    }

    let images_dir = output_dir.join("images");
    let target_dir = output_dir.join("target");
    let build_dir = output_dir.join("build");
    let host_dir = output_dir.join("host");
    if !images_dir.is_dir() || !target_dir.is_dir() || !build_dir.is_dir() || !host_dir.is_dir() {
        return Ok(None);
    }

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&script_value)
        .current_dir(buildroot_dir)
        .env("BASE_DIR", output_dir)
        .env("BINARIES_DIR", &images_dir)
        .env("TARGET_DIR", &target_dir)
        .env("BUILD_DIR", &build_dir)
        .env("HOST_DIR", &host_dir)
        .env("STAGING_DIR", output_dir.join("staging"))
        .env("BR2_CONFIG", &config_path);

    let mut messages = run_command(
        command,
        "buildroot direct post-image refresh",
        execution,
        policy,
        log_sink,
        cancel_check,
    )?;
    messages.push(format!(
        "refreshed buildroot post-image outputs directly via '{}'",
        script_value
    ));
    Ok(Some(messages))
}

fn buildroot_config_value(config: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    config.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?;
        Some(unquote_buildroot_value(value.trim()).to_string())
    })
}

fn unquote_buildroot_value(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn refresh_buildroot_squashfs_images_direct(
    buildroot_dir: &Path,
    output_dir: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
) -> Result<Option<Vec<String>>, ImageProviderError> {
    let normalized_output_dir = normalize_path(output_dir);
    let output_dir = normalized_output_dir.as_path();
    let rootfs_build_dir = output_dir.join("build/buildroot-fs/squashfs");
    let fakeroot_script = rootfs_build_dir.join("fakeroot");
    let staged_target_dir = rootfs_build_dir.join("target");
    let working_target_dir = rootfs_build_dir.join("target.refresh");
    let working_fakeroot_script = rootfs_build_dir.join("fakeroot.refresh");
    let devices_table = output_dir.join("build/buildroot-fs/full_devices_table.txt");
    let working_devices_table =
        output_dir.join("build/buildroot-fs/full_devices_table.refresh.txt");
    let source_target_dir = output_dir.join("target");
    let host_dir = output_dir.join("host");
    let fakeroot_bin = host_dir.join("bin/fakeroot");

    if !fakeroot_script.is_file()
        || !fakeroot_bin.is_file()
        || !source_target_dir.is_dir()
        || !host_dir.join("bin/mksquashfs").is_file()
    {
        return Ok(None);
    }

    if working_target_dir.exists() {
        fs::remove_dir_all(&working_target_dir).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to clean working squashfs target '{}': {error}",
                    working_target_dir.display()
                ),
            )
        })?;
    }
    if staged_target_dir.exists() {
        copy_path(&staged_target_dir, &working_target_dir)?;
        merge_tree_contents(&source_target_dir, &working_target_dir)?;
    } else {
        copy_path(&source_target_dir, &working_target_dir)?;
    }
    materialize_devices_table_for_target(
        &devices_table,
        &working_devices_table,
        &working_target_dir,
    )?;
    materialize_fakeroot_script_for_target(
        &fakeroot_script,
        &working_fakeroot_script,
        output_dir,
        &staged_target_dir,
        &working_target_dir,
        Some((&devices_table, &working_devices_table)),
    )?;
    ensure_fakeroot_chown_paths_exist(&working_fakeroot_script, &working_target_dir)?;
    seed_target_accounts_from_users_table(output_dir, &working_target_dir)?;

    let mut command = Command::new(&fakeroot_bin);
    let path_value = env::var_os("PATH").unwrap_or_default();
    let mut path_entries = vec![host_dir.join("bin"), host_dir.join("sbin")];
    path_entries.extend(env::split_paths(&path_value));
    let joined_path = env::join_paths(path_entries).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to build PATH for squashfs refresh: {error}"),
        )
    })?;

    command
        .arg("--")
        .arg(&working_fakeroot_script)
        .current_dir(buildroot_dir)
        .env("PATH", joined_path)
        .env("FAKEROOTDONTTRYCHOWN", "1");

    let mut messages = run_command(
        command,
        "buildroot direct squashfs refresh",
        execution,
        policy,
        None,
        None,
    )?;
    messages.push(format!(
        "refreshed squashfs image directly via '{}'",
        fakeroot_script.display()
    ));
    Ok(Some(messages))
}

pub(crate) fn ensure_fakeroot_chown_paths_exist(
    fakeroot_script: &Path,
    target_dir: &Path,
) -> Result<(), ImageProviderError> {
    let script = fs::read_to_string(fakeroot_script).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read fakeroot script '{}': {error}",
                fakeroot_script.display()
            ),
        )
    })?;

    for line in script.lines() {
        let line = line.trim();
        if !line.starts_with("chown ") {
            continue;
        }
        let Some(path_token) = line.split_whitespace().last() else {
            continue;
        };
        let target_path = path_token.trim_matches('\'').trim_matches('"');
        let target_path = Path::new(target_path);
        if !target_path.starts_with(target_dir) || target_path.exists() {
            continue;
        }
        fs::create_dir_all(target_path).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to precreate fakeroot chown path '{}': {error}",
                    target_path.display()
                ),
            )
        })?;
    }

    Ok(())
}

pub(crate) fn materialize_fakeroot_script_for_target(
    fakeroot_script: &Path,
    output_script: &Path,
    output_dir: &Path,
    original_target_dir: &Path,
    target_dir: &Path,
    devices_table_rewrite: Option<(&Path, &Path)>,
) -> Result<(), ImageProviderError> {
    let script = fs::read_to_string(fakeroot_script).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read fakeroot script '{}': {error}",
                fakeroot_script.display()
            ),
        )
    })?;
    let mut rewritten = script.clone();
    let current_output_dir = output_dir.display().to_string();
    let mut discovered_output_dirs = BTreeSet::new();
    for token in script.split_whitespace() {
        let token = token.trim_matches('\'').trim_matches('"');
        if !token.starts_with('/') {
            continue;
        }
        if let Some(marker) = token.find("/images/buildroot-output") {
            discovered_output_dirs
                .insert(token[..marker + "/images/buildroot-output".len()].to_string());
        }
    }
    for original_output_dir in discovered_output_dirs {
        rewritten = rewritten.replace(&original_output_dir, &current_output_dir);
    }
    rewritten = rewritten.replace(
        &original_target_dir.display().to_string(),
        &target_dir.display().to_string(),
    );
    let rewritten =
        if let Some((original_devices_table, rewritten_devices_table)) = devices_table_rewrite {
            rewritten.replace(
                &original_devices_table.display().to_string(),
                &rewritten_devices_table.display().to_string(),
            )
        } else {
            rewritten
        };
    fs::write(output_script, rewritten).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write rewritten fakeroot script '{}': {error}",
                output_script.display()
            ),
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(output_script, fs::Permissions::from_mode(0o755)).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to mark rewritten fakeroot script '{}' executable: {error}",
                output_script.display()
            ),
        )
    })?;
    Ok(())
}

pub(crate) fn materialize_devices_table_for_target(
    devices_table: &Path,
    output_table: &Path,
    target_dir: &Path,
) -> Result<(), ImageProviderError> {
    if !devices_table.is_file() {
        return Ok(());
    }

    let mut users = BTreeMap::new();
    let passwd_path = target_dir.join("etc/passwd");
    let group_path = target_dir.join("etc/group");
    if passwd_path.is_file() {
        for line in fs::read_to_string(&passwd_path)
            .map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read passwd file '{}': {error}",
                        passwd_path.display()
                    ),
                )
            })?
            .lines()
        {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 4 {
                users.insert(fields[0].to_string(), fields[2].to_string());
            }
        }
    }
    let mut groups = BTreeMap::new();
    if group_path.is_file() {
        for line in fs::read_to_string(&group_path)
            .map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read group file '{}': {error}",
                        group_path.display()
                    ),
                )
            })?
            .lines()
        {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 {
                groups.insert(fields[0].to_string(), fields[2].to_string());
            }
        }
    }

    let mut rewritten = String::new();
    for raw_line in fs::read_to_string(devices_table)
        .map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read devices table '{}': {error}",
                    devices_table.display()
                ),
            )
        })?
        .lines()
    {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            rewritten.push_str(raw_line);
            rewritten.push('\n');
            continue;
        }
        let mut fields: Vec<String> = raw_line
            .split_whitespace()
            .map(|value| value.to_string())
            .collect();
        if fields.len() >= 5 {
            if let Some(uid) = users.get(&fields[3]) {
                fields[3] = uid.clone();
            }
            if let Some(gid) = groups.get(&fields[4]) {
                fields[4] = gid.clone();
            }
        }
        rewritten.push_str(&fields.join(" "));
        rewritten.push('\n');
    }

    fs::write(output_table, rewritten).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write rewritten devices table '{}': {error}",
                output_table.display()
            ),
        )
    })?;

    Ok(())
}

pub(crate) fn seed_target_accounts_from_users_table(
    output_dir: &Path,
    target_dir: &Path,
) -> Result<(), ImageProviderError> {
    let users_table_path = output_dir.join("build/buildroot-fs/full_users_table.txt");
    let group_path = target_dir.join("etc/group");
    let passwd_path = target_dir.join("etc/passwd");
    if !users_table_path.is_file() || !group_path.is_file() || !passwd_path.is_file() {
        return Ok(());
    }

    let users_table = fs::read_to_string(&users_table_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read users table '{}': {error}",
                users_table_path.display()
            ),
        )
    })?;
    let mut group_contents = fs::read_to_string(&group_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read group file '{}': {error}",
                group_path.display()
            ),
        )
    })?;
    let mut passwd_contents = fs::read_to_string(&passwd_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read passwd file '{}': {error}",
                passwd_path.display()
            ),
        )
    })?;

    let mut groups = BTreeMap::new();
    let mut max_gid = 99u32;
    for line in group_contents.lines() {
        let mut fields = line.split(':');
        let Some(name) = fields.next() else { continue };
        let _passwd = fields.next();
        let gid = fields
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(max_gid);
        groups.insert(name.to_string(), gid);
        max_gid = max_gid.max(gid);
    }

    let mut users = BTreeMap::new();
    let mut max_uid = 99u32;
    for line in passwd_contents.lines() {
        let mut fields = line.split(':');
        let Some(name) = fields.next() else { continue };
        let _passwd = fields.next();
        let uid = fields
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(max_uid);
        users.insert(name.to_string(), uid);
        max_uid = max_uid.max(uid);
    }

    for raw_line in users_table.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let user_name = fields[0];
        let uid_spec = fields[1];
        let group_name = fields[2];
        let gid_spec = fields[3];
        let home = fields[5];
        let shell = fields[6];
        let gecos = if fields.len() > 7 {
            fields[7..].join(" ")
        } else {
            String::new()
        };

        if group_name != "-" && !groups.contains_key(group_name) {
            let gid = gid_spec
                .parse::<u32>()
                .ok()
                .filter(|gid| *gid != u32::MAX)
                .unwrap_or_else(|| {
                    max_gid += 1;
                    max_gid
                });
            if !group_contents.ends_with('\n') {
                group_contents.push('\n');
            }
            group_contents.push_str(&format!("{group_name}:x:{gid}:\n"));
            groups.insert(group_name.to_string(), gid);
            max_gid = max_gid.max(gid);
        }

        if user_name != "-" && !users.contains_key(user_name) {
            let uid = uid_spec
                .parse::<u32>()
                .ok()
                .filter(|uid| *uid != u32::MAX)
                .unwrap_or_else(|| {
                    max_uid += 1;
                    max_uid
                });
            let gid = groups.get(group_name).copied().unwrap_or(0);
            let home = if home == "-" { "/" } else { home };
            let shell = if shell == "-" { "/bin/false" } else { shell };
            if !passwd_contents.ends_with('\n') {
                passwd_contents.push('\n');
            }
            passwd_contents.push_str(&format!(
                "{user_name}:x:{uid}:{gid}:{gecos}:{home}:{shell}\n"
            ));
            users.insert(user_name.to_string(), uid);
            max_uid = max_uid.max(uid);
        }
    }

    fs::write(&group_path, group_contents).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to update group file '{}': {error}",
                group_path.display()
            ),
        )
    })?;
    fs::write(&passwd_path, passwd_contents).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to update passwd file '{}': {error}",
                passwd_path.display()
            ),
        )
    })?;

    Ok(())
}
