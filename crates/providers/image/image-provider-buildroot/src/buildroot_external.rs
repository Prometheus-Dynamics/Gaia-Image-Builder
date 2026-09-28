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
    pub replacement_digest: Option<String>,
}

pub(crate) fn materialize_buildroot_package_overrides(
    spec: &ResolvedBuildSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
) -> Result<MaterializedBuildrootPackageOverrides, ImageProviderError> {
    let package_override_dirs = buildroot_package_override_dirs(spec);
    if package_override_dirs.is_empty() {
        return Ok(MaterializedBuildrootPackageOverrides {
            generated_external_tree: None,
            replacement_count: 0,
            replacement_digest: None,
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

    let generated_external_tree = if package_names.is_empty() {
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
        write_generated_external_tree_metadata(&external_tree_dir, &package_names)?;
        Some(GeneratedBuildrootExternalTree {
            path: external_tree_dir,
            package_count: package_names.len(),
        })
    };
    Ok(MaterializedBuildrootPackageOverrides {
        generated_external_tree,
        replacement_count: replacement_names.len(),
        replacement_digest: (!replacement_names.is_empty())
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

fn write_generated_external_tree_metadata(
    external_tree_dir: &Path,
    package_names: &[String],
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
            "include $(sort $(wildcard $(BR2_EXTERNAL_{GENERATED_EXTERNAL_NAME}_PATH)/package/*/*.mk))\n"
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
