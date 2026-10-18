use super::*;

const GENERATED_EXTERNAL_NAME: &str = "GAIA_GENERATED";
const GENERATED_EXTERNAL_DESC: &str =
    "name: GAIA_GENERATED\ndesc: Gaia generated Buildroot package overrides\n";

pub(crate) struct GeneratedBuildrootExternalTree {
    pub path: PathBuf,
    pub package_count: usize,
}

pub(crate) struct MaterializedBuildrootPackageOverrides {
    pub generated_external_tree: Option<GeneratedBuildrootExternalTree>,
    pub replacement_count: usize,
    /// Content digest of the override trees.
    pub replacement_digest: Option<String>,
    /// The timestamp-based digest earlier Gaia versions stored; still
    /// accepted so an upgrade does not force a full clean.
    pub legacy_replacement_digest: Option<String>,
}

pub(crate) fn materialize_buildroot_package_overrides(
    spec: &ResolvedBuildSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
) -> Result<MaterializedBuildrootPackageOverrides, ImageProviderError> {
    let package_override_dirs = buildroot_package_override_dirs(spec);
    let erofs_lines = erofs_make_lines(spec);
    if package_override_dirs.is_empty() && erofs_lines.is_none() {
        return Ok(MaterializedBuildrootPackageOverrides {
            generated_external_tree: None,
            replacement_count: 0,
            replacement_digest: None,
            legacy_replacement_digest: None,
        });
    }

    let external_tree_dir = output_dir.join("gaia-buildroot-external");
    let external_package_dir = external_tree_dir.join("package");
    if external_tree_dir.exists() {
        fs::remove_dir_all(&external_tree_dir).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to clean generated Buildroot external tree '{}': {error}",
                external_tree_dir.display()
            ))
        })?;
    }
    fs::create_dir_all(&external_package_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create generated Buildroot external package dir '{}': {error}",
            external_package_dir.display()
        ))
    })?;
    let mut package_names = Vec::new();
    let mut replacement_names = Vec::new();
    for package_override_dir in &package_override_dirs {
        for entry in fs::read_dir(package_override_dir).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read Buildroot package overrides '{}': {error}",
                package_override_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to read Buildroot package override entry in '{}': {error}",
                    package_override_dir.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to inspect Buildroot package override '{}': {error}",
                    entry.path().display()
                ))
            })?;
            if !file_type.is_dir() {
                continue;
            }

            let package_name = entry.file_name().into_string().map_err(|name| {
                ImageProviderError::backend_command(format!(
                    "Buildroot package override name '{}' is not valid UTF-8",
                    name.to_string_lossy()
                ))
            })?;
            validate_package_override(&entry.path(), &package_name)?;
            let buildroot_package_dir = buildroot_dir.join("package").join(&package_name);
            if buildroot_package_dir.is_dir() {
                fs::remove_dir_all(&buildroot_package_dir).map_err(|error| {
                    ImageProviderError::backend_command(format!(
                        "failed to replace Buildroot package '{}' at '{}': {error}",
                        package_name,
                        buildroot_package_dir.display()
                    ))
                })?;
                copy_dir_contents(&entry.path(), &buildroot_package_dir, None)?;
                replacement_names.push(package_name);
            } else {
                let dest = external_package_dir.join(&package_name);
                copy_dir_contents(&entry.path(), &dest, None)?;
                package_names.push(package_name);
            }
        }
    }
    package_names.sort();
    replacement_names.sort();

    let generated_external_tree = if package_names.is_empty() && erofs_lines.is_none() {
        if external_tree_dir.exists() {
            fs::remove_dir_all(&external_tree_dir).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to clean empty generated Buildroot external tree '{}': {error}",
                    external_tree_dir.display()
                ))
            })?;
        }
        None
    } else {
        write_generated_external_tree_metadata(
            &external_tree_dir,
            &package_names,
            erofs_lines.as_deref(),
        )?;
        Some(GeneratedBuildrootExternalTree {
            path: external_tree_dir,
            package_count: package_names.len(),
        })
    };
    Ok(MaterializedBuildrootPackageOverrides {
        generated_external_tree,
        replacement_count: replacement_names.len(),
        replacement_digest: (!replacement_names.is_empty())
            .then(|| package_override_content_digest(Some(output_dir), &package_override_dirs)),
        legacy_replacement_digest: (!replacement_names.is_empty())
            .then(|| package_override_dirs_digest(&package_override_dirs)),
    })
}

pub(crate) fn buildroot_package_override_dirs(spec: &ResolvedBuildSpec) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let workspace_root = Path::new(&spec.workspace.root_dir);
    let legacy = workspace_root.join("gaia/assets/buildroot/packages");
    if legacy.is_dir() {
        dirs.push(legacy);
    }
    if let Some(external_tree) = configured_external_tree(spec) {
        for tree in external_tree
            .split(':')
            .map(str::trim)
            .filter(|tree| !tree.is_empty())
        {
            let tree_path = resolve_workspace_relative(workspace_root, tree);
            let package_dir = tree_path.join("packages");
            if package_dir.is_dir() {
                dirs.push(package_dir);
            }
        }
    }
    dirs
}

fn configured_external_tree(spec: &ResolvedBuildSpec) -> Option<&str> {
    match &spec.image.definition {
        gaia_spec::ImageDefinition::Buildroot(buildroot) => buildroot.external_tree.as_deref(),
        _ => None,
    }
}

fn resolve_workspace_relative(workspace_root: &Path, path: &str) -> PathBuf {
    let raw = Path::new(path);
    if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        workspace_root.join(raw)
    }
}

fn package_override_dirs_digest(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|dir| format!("{}={}", dir.display(), dir_digest(dir)))
        .collect::<Vec<_>>()
        .join(";")
}

/// Digest standing for the `mkfs.erofs` multithreading settings in the
/// generated tree. The tree's `external.mk` is not a configured external
/// tree, so its settings reach package keys and rebuilds through this.
const EROFS_MULTITHREADING_DIGEST: &str = "erofs-multithreading:v1";

/// The Gaia-written settings that change a package's build, by package name
/// and a digest of them (for package keys and the rebuild record). Empty when
/// none apply.
pub(crate) fn generated_package_digests(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    let mut digests = BTreeMap::new();
    if erofs_workers_setting(spec) != gaia_spec::ErofsWorkersSpec::Off {
        digests.insert(
            "host-erofs-utils".to_string(),
            EROFS_MULTITHREADING_DIGEST.to_string(),
        );
    }
    digests
}

/// The buildroot policy's `erofs_workers` (Off for other image providers).
fn erofs_workers_setting(spec: &ResolvedBuildSpec) -> gaia_spec::ErofsWorkersSpec {
    if spec.image.provider_kind() == gaia_spec::ImageProviderKind::Buildroot {
        spec.policy.providers.buildroot.erofs_workers
    } else {
        gaia_spec::ErofsWorkersSpec::Off
    }
}

/// The lines of the generated `external.mk` that build erofs-utils with
/// multithreading and pass `--workers` to `mkfs.erofs` on EROFS rootfs
/// images. `None` when `erofs_workers` is off. `auto` is the smaller of 8 and
/// the host's cores, evaluated by make, so the file is the same on every host.
/// The image is byte-identical for any worker count.
pub(crate) fn erofs_make_lines(spec: &ResolvedBuildSpec) -> Option<String> {
    let workers = match erofs_workers_setting(spec) {
        gaia_spec::ErofsWorkersSpec::Off => return None,
        gaia_spec::ErofsWorkersSpec::Auto => {
            "GAIA_EROFS_WORKERS ?= $(shell n=$$(nproc 2>/dev/null || echo 1); \
             if [ $$n -gt 8 ]; then echo 8; else echo $$n; fi)"
                .to_string()
        }
        gaia_spec::ErofsWorkersSpec::Count(count) => format!("GAIA_EROFS_WORKERS = {count}"),
    };
    Some(format!(
        "# Written by Gaia (erofs_workers).\n{workers}\n\
         ifeq ($(BR2_TARGET_ROOTFS_EROFS),y)\n\
         HOST_EROFS_UTILS_CONF_OPTS += --enable-multithreading\n\
         ROOTFS_EROFS_ARGS += --workers=$(GAIA_EROFS_WORKERS)\n\
         endif\n"
    ))
}

fn write_generated_external_tree_metadata(
    external_tree_dir: &Path,
    package_names: &[String],
    erofs_lines: Option<&str>,
) -> Result<(), ImageProviderError> {
    fs::write(
        external_tree_dir.join("external.desc"),
        GENERATED_EXTERNAL_DESC,
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write generated Buildroot external.desc '{}': {error}",
            external_tree_dir.join("external.desc").display()
        ))
    })?;
    let config_in = package_names
        .iter()
        .map(|package| {
            format!("source \"$BR2_EXTERNAL_{GENERATED_EXTERNAL_NAME}_PATH/package/{package}/Config.in\"\n")
        })
        .collect::<String>();
    fs::write(external_tree_dir.join("Config.in"), config_in).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write generated Buildroot external Config.in '{}': {error}",
            external_tree_dir.join("Config.in").display()
        ))
    })?;
    fs::write(
        external_tree_dir.join("external.mk"),
        format!(
            "include $(sort $(wildcard $(BR2_EXTERNAL_{GENERATED_EXTERNAL_NAME}_PATH)/package/*/*.mk))\n{}",
            erofs_lines.unwrap_or_default()
        ),
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write generated Buildroot external.mk '{}': {error}",
            external_tree_dir.join("external.mk").display()
        ))
    })
}

fn validate_package_override(path: &Path, package_name: &str) -> Result<(), ImageProviderError> {
    if !package_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ImageProviderError::backend_command(format!(
            "Buildroot package override '{}' has an invalid directory name; use only ASCII letters, digits, '.', '_', or '-'",
            path.display()
        )));
    }
    let config_in = path.join("Config.in");
    if !config_in.is_file() {
        return Err(ImageProviderError::backend_command(format!(
            "Buildroot package override '{}' is missing required Config.in",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn buildroot_external_tree_value(
    spec: &ResolvedBuildSpec,
    configured: Option<&str>,
    generated: Option<&Path>,
) -> Option<String> {
    let mut trees = Vec::new();
    if let Some(configured) = configured
        && !configured.trim().is_empty()
    {
        let workspace_root = Path::new(&spec.workspace.root_dir);
        trees.extend(
            configured
                .split(':')
                .map(str::trim)
                .filter(|tree| !tree.is_empty())
                .map(|tree| {
                    resolve_workspace_relative(workspace_root, tree)
                        .display()
                        .to_string()
                }),
        );
    }
    if let Some(generated) = generated {
        trees.push(generated.display().to_string());
    }
    (!trees.is_empty()).then(|| trees.join(":"))
}

pub(crate) fn ensure_no_generated_external_name_conflict(
    configured: Option<&str>,
) -> Result<(), ImageProviderError> {
    let Some(configured) = configured else {
        return Ok(());
    };
    for external_tree in configured
        .split(':')
        .map(str::trim)
        .filter(|tree| !tree.is_empty())
    {
        let desc = Path::new(external_tree).join("external.desc");
        let Ok(contents) = fs::read_to_string(&desc) else {
            continue;
        };
        if contents
            .lines()
            .any(|line| line.trim() == format!("name: {GENERATED_EXTERNAL_NAME}"))
        {
            return Err(ImageProviderError::backend_command(format!(
                "configured Buildroot external tree '{}' uses reserved generated external name {GENERATED_EXTERNAL_NAME}",
                Path::new(external_tree).display()
            )));
        }
    }
    Ok(())
}
