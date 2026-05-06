use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;
use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn interpolate_string(
    value: String,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> String {
    let mut output = String::new();
    let mut rest = value.as_str();

    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let token = &remainder[..end];
        output.push_str(&resolve_token(token, raw, env));
        rest = &remainder[end + 1..];
    }

    output.push_str(rest);
    output
}

fn resolve_token(token: &str, raw: &RawBuildConfig, env: &ResolvedEnvironment) -> String {
    if let Some(key) = token.strip_prefix("env:") {
        return env.get(key).unwrap_or_default().to_string();
    }

    match token {
        "build.name" => interpolate_string(raw.build_name.clone(), raw, env),
        "build.display_name" => raw
            .display_name
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_else(|| interpolate_string(raw.build_name.clone(), raw, env)),
        "build.version" => raw
            .version
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "build.description" => raw
            .description
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "build.branch" => raw
            .branch
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "build.target" => raw
            .target
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "build.profile" => raw
            .profile
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "preset.name" => raw.preset.clone().unwrap_or_default(),
        _ if token.starts_with("input.") || token.starts_with("inputs.") => {
            let name = token
                .strip_prefix("input.")
                .or_else(|| token.strip_prefix("inputs."))
                .unwrap_or_default();
            raw.selected_inputs
                .iter()
                .find_map(|(input_name, value)| {
                    if input_name == name {
                        Some(interpolate_string(value.clone(), raw, env))
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        }
        "product.family" => raw
            .product
            .family
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "product.name" => raw
            .product
            .name
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "product.sku" => raw
            .product
            .sku
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "provenance.identity.project" => raw
            .provenance
            .identity
            .project
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "provenance.identity.vendor" => raw
            .provenance
            .identity
            .vendor
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "provenance.identity.channel" => raw
            .provenance
            .identity
            .channel
            .clone()
            .map(|value| interpolate_string(value, raw, env))
            .unwrap_or_default(),
        "config.root_dir" => config_root_dir(raw),
        "execution.root_dir" => execution_root_dir(),
        "project.root_dir" => project_root_dir(raw, env),
        "workspace.root_dir" => interpolate_string(raw.workspace.root_dir.clone(), raw, env),
        "workspace.build_dir" => interpolate_string(raw.workspace.build_dir.clone(), raw, env),
        "workspace.out_dir" => interpolate_string(raw.workspace.out_dir.clone(), raw, env),
        _ => raw
            .interpolation
            .values
            .iter()
            .find_map(|(name, value)| {
                if token == format!("interpolation.values.{name}") {
                    Some(interpolate_string(value.clone(), raw, env))
                } else {
                    None
                }
            })
            .or_else(|| {
                raw.labels.iter().find_map(|(name, value)| {
                    if token == format!("build.labels.{name}") {
                        Some(interpolate_string(value.clone(), raw, env))
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                raw.provenance
                    .identity
                    .labels
                    .iter()
                    .find_map(|(name, value)| {
                        if token == format!("provenance.identity.labels.{name}") {
                            Some(interpolate_string(value.clone(), raw, env))
                        } else {
                            None
                        }
                    })
            })
            .or_else(|| {
                raw.workspace.named_paths.iter().find_map(|entry| {
                    if token == format!("workspace.paths.{}", entry.alias) {
                        Some(interpolate_string(entry.path.clone(), raw, env))
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| format!("${{{token}}}")),
    }
}

fn config_root_dir(raw: &RawBuildConfig) -> String {
    raw.source_path
        .as_deref()
        .and_then(Path::parent)
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn execution_root_dir() -> String {
    env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn project_root_dir(raw: &RawBuildConfig, env: &ResolvedEnvironment) -> String {
    let build_root = raw
        .source_path
        .as_deref()
        .and_then(config_workspace_root)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let workspace_root = interpolate_string(raw.workspace.root_dir.clone(), raw, env);
    normalize_path(&absolutize(&build_root, &workspace_root))
        .display()
        .to_string()
}

fn config_workspace_root(source_path: &Path) -> Option<PathBuf> {
    for ancestor in source_path.ancestors().skip(1) {
        if ancestor.join("Cargo.toml").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn absolutize(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        normalized.push(component.as_os_str());
    }
    normalized
}
