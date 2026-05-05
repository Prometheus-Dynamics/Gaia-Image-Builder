use super::*;

const GENERATED_EXTERNAL_NAME: &str = "GAIA_GENERATED";
const GENERATED_EXTERNAL_DESC: &str =
    "name: GAIA_GENERATED\ndesc: Gaia generated Buildroot package overrides\n";

pub(crate) struct GeneratedBuildrootExternalTree {
    pub path: PathBuf,
    pub package_count: usize,
}

pub(crate) fn materialize_buildroot_package_overrides(
    spec: &ResolvedBuildSpec,
    output_dir: &Path,
) -> Result<Option<GeneratedBuildrootExternalTree>, ImageProviderError> {
    let package_override_dir =
        Path::new(&spec.workspace.root_dir).join("gaia/assets/buildroot/packages");
    if !package_override_dir.is_dir() {
        return Ok(None);
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
    for entry in fs::read_dir(&package_override_dir).map_err(|error| {
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
        let dest = external_package_dir.join(&package_name);
        copy_dir_contents(&entry.path(), &dest, None)?;
        package_names.push(package_name);
    }
    package_names.sort();

    write_generated_external_tree_metadata(&external_tree_dir, &package_names)?;
    Ok(Some(GeneratedBuildrootExternalTree {
        path: external_tree_dir,
        package_count: package_names.len(),
    }))
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
    configured: Option<&str>,
    generated: Option<&Path>,
) -> Option<String> {
    let mut trees = Vec::new();
    if let Some(configured) = configured
        && !configured.trim().is_empty()
    {
        trees.push(configured.to_string());
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
