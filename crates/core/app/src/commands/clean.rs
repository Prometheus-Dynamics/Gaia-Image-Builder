use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{CleanProfileSpec, ResolvedBuildSpec};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::CleanArgs;

use super::CommandOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanReport {
    pub build_name: String,
    pub dry_run: bool,
    pub removed: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
}

pub fn clean_build_command(
    build: &str,
    options: &ResolveOptions,
    clean_args: &CleanArgs,
) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };

    match clean_build(&spec, clean_args) {
        Ok(report) => CommandOutcome::Cleaned { spec, report },
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn clean_build(spec: &ResolvedBuildSpec, clean_args: &CleanArgs) -> Result<CleanReport, String> {
    let paths = clean_paths(spec, clean_args)?;
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for path in paths {
        guard_clean_path(spec, &path)?;
        if !path.exists() {
            missing.push(path);
            continue;
        }
        if !clean_args.dry_run {
            remove_path(&path).map_err(|error| {
                format!(
                    "failed to clean '{}' for build '{}': {error}",
                    path.display(),
                    spec.identity.display_name
                )
            })?;
        }
        removed.push(path);
    }

    Ok(CleanReport {
        build_name: spec.identity.display_name.clone(),
        dry_run: clean_args.dry_run,
        removed,
        missing,
    })
}

fn clean_paths(spec: &ResolvedBuildSpec, clean_args: &CleanArgs) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();

    if let Some(profile_name) = clean_args.profile.as_deref() {
        append_profile_paths(spec, profile_name, &mut paths)?;
    } else if clean_args.targets.is_empty() && clean_args.paths.is_empty() {
        if let Some(profile_name) = spec.clean.default_profile.as_deref() {
            append_profile_paths(spec, profile_name, &mut paths)?;
        } else {
            paths.push(PathBuf::from(&spec.workspace.build_dir));
            paths.push(PathBuf::from(&spec.workspace.out_dir));
        }
    }

    for target in &clean_args.targets {
        append_target_paths(spec, target, &mut paths)?;
    }

    for path in &clean_args.paths {
        paths.push(spec.workspace.resolve_path(path).map_err(|error| {
            format!(
                "invalid clean path '{}' for build '{}': {error}",
                path, spec.identity.display_name
            )
        })?);
    }

    Ok(dedupe_paths(paths))
}

fn append_profile_paths(
    spec: &ResolvedBuildSpec,
    profile_name: &str,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let profile = spec.clean.profiles.get(profile_name).ok_or_else(|| {
        format!(
            "unknown clean profile '{}' for build '{}'",
            profile_name, spec.identity.display_name
        )
    })?;
    append_profile(spec, profile, paths)
}

fn append_profile(
    spec: &ResolvedBuildSpec,
    profile: &CleanProfileSpec,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if profile.build {
        paths.push(PathBuf::from(&spec.workspace.build_dir));
    }
    if profile.out {
        paths.push(PathBuf::from(&spec.workspace.out_dir));
    }
    for path in &profile.paths {
        paths.push(spec.workspace.resolve_path(path).map_err(|error| {
            format!(
                "invalid configured clean path '{}' for build '{}': {error}",
                path, spec.identity.display_name
            )
        })?);
    }
    Ok(())
}

fn append_target_paths(
    spec: &ResolvedBuildSpec,
    target: &str,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    match target {
        "build" => paths.push(PathBuf::from(&spec.workspace.build_dir)),
        "out" | "outputs" => paths.push(PathBuf::from(&spec.workspace.out_dir)),
        "all" => {
            paths.push(PathBuf::from(&spec.workspace.build_dir));
            paths.push(PathBuf::from(&spec.workspace.out_dir));
        }
        "configured" => {
            let Some(profile_name) = spec.clean.default_profile.as_deref() else {
                return Err(format!(
                    "clean target 'configured' requires clean.default for build '{}'",
                    spec.identity.display_name
                ));
            };
            append_profile_paths(spec, profile_name, paths)?;
        }
        value => {
            return Err(format!(
                "unknown clean target '{}' for build '{}'",
                value, spec.identity.display_name
            ));
        }
    }
    Ok(())
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            deduped.push(path);
        }
    }
    deduped
}

fn guard_clean_path(spec: &ResolvedBuildSpec, path: &Path) -> Result<(), String> {
    let workspace_root = Path::new(&spec.workspace.root_dir);
    if path == workspace_root {
        return Err(format!(
            "refusing to clean workspace root '{}' for build '{}'",
            path.display(),
            spec.identity.display_name
        ));
    }
    if path.parent().is_none() {
        return Err(format!(
            "refusing to clean unsafe path '{}' for build '{}'",
            path.display(),
            spec.identity.display_name
        ));
    }
    Ok(())
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
