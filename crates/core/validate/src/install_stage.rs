use std::collections::{HashMap, HashSet};
use std::fs;

use gaia_spec::{ResolvedBuildSpec, StageContentOriginSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};
use crate::workspace::resolve_workspace_path;

pub(crate) fn validate_install_and_stage(
    spec: &ResolvedBuildSpec,
    artifact_ids: &HashSet<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut install_ids = HashSet::new();
    for install in &spec.install.entries {
        if !install.id.is_valid() {
            diagnostics.push(error(
                "install_id_empty",
                "install id cannot be empty".into(),
                Some("install".into()),
            ));
        }
        if !install_ids.insert(install.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_install_id",
                format!("duplicate install id '{}'", install.id.as_str()),
                Some(format!("install:{}", install.id.as_str())),
            ));
        }
        if !artifact_ids.contains(install.artifact.id.as_str()) {
            diagnostics.push(error(
                "unknown_install_artifact",
                format!(
                    "install '{}' references unknown artifact '{}'",
                    install.id.as_str(),
                    install.artifact.id.as_str()
                ),
                Some(format!("install:{}", install.id.as_str())),
            ));
        }
        if !install.dest.starts_with('/') {
            diagnostics.push(error(
                "install_dest_not_absolute",
                format!(
                    "install '{}' has non-absolute destination '{}'",
                    install.id.as_str(),
                    install.dest
                ),
                Some(format!("install:{}", install.id.as_str())),
            ));
        }
    }

    let mut stage_ids = HashSet::new();
    for file in &spec.stage.files {
        if !file.id.is_valid() {
            diagnostics.push(error(
                "stage_item_id_empty",
                "stage file id cannot be empty".into(),
                Some("stage-file".into()),
            ));
        }
        if !stage_ids.insert(file.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_stage_item_id",
                format!("duplicate stage item id '{}'", file.id.as_str()),
                Some(format!("stage:{}", file.id.as_str())),
            ));
        }
        if file.src.trim().is_empty() || file.dest.trim().is_empty() {
            diagnostics.push(error(
                "stage_file_path_empty",
                format!(
                    "stage file '{}' must have both src and dest",
                    file.id.as_str()
                ),
                Some(format!("stage:{}", file.id.as_str())),
            ));
        }
        if !file.dest.starts_with('/') {
            diagnostics.push(error(
                "stage_file_dest_not_absolute",
                format!(
                    "stage file '{}' has non-absolute destination '{}'",
                    file.id.as_str(),
                    file.dest
                ),
                Some(format!("stage:{}", file.id.as_str())),
            ));
        }
        if file.origin == StageContentOriginSpec::Generated
            && (file.src.contains("/assets/") || file.src.starts_with("assets/"))
        {
            diagnostics.push(error(
                "stage_file_generated_static_conflict",
                format!(
                    "stage file '{}' is marked generated but points at static asset path '{}'",
                    file.id.as_str(),
                    file.src
                ),
                Some(format!("stage:{}", file.id.as_str())),
            ));
        }
        if file.origin == StageContentOriginSpec::StaticAsset {
            match resolve_workspace_path(spec, &file.src) {
                Ok(resolved_path) => {
                    if !resolved_path.exists() {
                        diagnostics.push(error(
                            "stage_file_src_missing",
                            format!(
                                "stage file '{}' source '{}' does not resolve to an existing path",
                                file.id.as_str(),
                                resolved_path.display()
                            ),
                            Some(format!("stage:{}", file.id.as_str())),
                        ));
                    } else if resolved_path.is_file()
                        && should_check_busybox_script(file.dest.as_str(), &resolved_path)
                        && let Ok(contents) = fs::read_to_string(&resolved_path)
                    {
                        diagnostics.extend(validate_busybox_script_compatibility(
                            &contents,
                            file.id.as_str(),
                            &resolved_path,
                        ));
                    }
                }
                Err(message) => diagnostics.push(error(
                    "stage_file_src_invalid",
                    message,
                    Some(format!("stage:{}", file.id.as_str())),
                )),
            }
        }
    }
    for env in &spec.stage.env_sets {
        if !env.id.is_valid() {
            diagnostics.push(error(
                "stage_item_id_empty",
                "stage env set id cannot be empty".into(),
                Some("stage-env".into()),
            ));
        }
        if !stage_ids.insert(env.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_stage_item_id",
                format!("duplicate stage item id '{}'", env.id.as_str()),
                Some(format!("stage:{}", env.id.as_str())),
            ));
        }
        if env.name.trim().is_empty() {
            diagnostics.push(error(
                "stage_env_name_empty",
                format!("stage env set '{}' has an empty name", env.id.as_str()),
                Some(format!("stage:{}", env.id.as_str())),
            ));
        }
    }
    for service in &spec.stage.services {
        if !service.id.is_valid() {
            diagnostics.push(error(
                "stage_item_id_empty",
                "stage service id cannot be empty".into(),
                Some("stage-service".into()),
            ));
        }
        if !stage_ids.insert(service.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_stage_item_id",
                format!("duplicate stage item id '{}'", service.id.as_str()),
                Some(format!("stage:{}", service.id.as_str())),
            ));
        }
        if service.name.trim().is_empty() || service.unit_path.trim().is_empty() {
            diagnostics.push(error(
                "stage_service_empty",
                format!(
                    "stage service '{}' must have both name and unit path",
                    service.id.as_str()
                ),
                Some(format!("stage:{}", service.id.as_str())),
            ));
        }
        if !is_systemd_unit_name(&service.name) {
            diagnostics.push(warning(
                "stage_service_name_unusual",
                format!(
                    "stage service '{}' does not end with a known systemd unit suffix",
                    service.id.as_str()
                ),
                Some(format!("stage:{}", service.id.as_str())),
            ));
        }
        match resolve_workspace_path(spec, &service.unit_path) {
            Ok(resolved_path) => {
                if !resolved_path.is_file() {
                    diagnostics.push(error(
                    "stage_service_unit_missing",
                    format!(
                        "stage service '{}' unit_path '{}' does not resolve to an existing file",
                        service.id.as_str(),
                        resolved_path.display()
                    ),
                    Some(format!("stage:{}", service.id.as_str())),
                ));
                }
            }
            Err(message) => diagnostics.push(error(
                "stage_service_unit_invalid",
                message,
                Some(format!("stage:{}", service.id.as_str())),
            )),
        }
    }

    let mut image_destinations: HashMap<String, String> = HashMap::new();
    for install in &spec.install.entries {
        register_image_destination(
            diagnostics,
            &mut image_destinations,
            install.dest.as_str(),
            format!("install:{}", install.id.as_str()),
            format!("install '{}'", install.id.as_str()),
        );
    }
    for file in &spec.stage.files {
        register_image_destination(
            diagnostics,
            &mut image_destinations,
            file.dest.as_str(),
            format!("stage:{}", file.id.as_str()),
            format!("stage file '{}'", file.id.as_str()),
        );
    }
    for env in &spec.stage.env_sets {
        register_image_destination(
            diagnostics,
            &mut image_destinations,
            format!("/etc/default/{}.env", env.name),
            format!("stage:{}", env.id.as_str()),
            format!("stage env set '{}'", env.id.as_str()),
        );
    }
    for service in &spec.stage.services {
        register_image_destination(
            diagnostics,
            &mut image_destinations,
            format!("/etc/systemd/system/{}", service.name),
            format!("stage:{}", service.id.as_str()),
            format!("stage service '{}'", service.id.as_str()),
        );
    }
}

fn register_image_destination(
    diagnostics: &mut Vec<ValidationDiagnostic>,
    image_destinations: &mut HashMap<String, String>,
    dest: impl Into<String>,
    location: String,
    owner: String,
) {
    let dest = dest.into();
    let existing = image_destinations.insert(dest.clone(), owner.clone());
    if let Some(existing_owner) = existing {
        diagnostics.push(error(
            "image_destination_conflict",
            format!(
                "image destination '{}' is claimed by both {} and {}",
                dest, existing_owner, owner
            ),
            Some(location),
        ));
    }
}

fn is_systemd_unit_name(name: &str) -> bool {
    [
        ".automount",
        ".device",
        ".mount",
        ".path",
        ".scope",
        ".service",
        ".slice",
        ".socket",
        ".swap",
        ".target",
        ".timer",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn should_check_busybox_script(dest: &str, resolved_path: &std::path::Path) -> bool {
    dest.starts_with("/etc/init.d/")
        || dest.ends_with(".sh")
        || resolved_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('S') || name.ends_with(".sh"))
}

fn validate_busybox_script_compatibility(
    contents: &str,
    stage_id: &str,
    resolved_path: &std::path::Path,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    for (line_number, line) in contents.lines().enumerate() {
        if line.contains("wget") && line.contains("--method") {
            diagnostics.push(warning(
                "stage_script_busybox_portability_risk",
                format!(
                    "stage file '{}' uses 'wget --method' on line {} in '{}'; BusyBox wget does not support that flag",
                    stage_id,
                    line_number + 1,
                    resolved_path.display()
                ),
                Some(format!("stage:{stage_id}")),
            ));
        }
        if line.contains("awk")
            && (line.contains("{2,}")
                || line.contains("{1,}")
                || line.contains("{1}")
                || line.contains("{2}")
                || line.contains("{3}"))
        {
            diagnostics.push(warning(
                "stage_script_busybox_portability_risk",
                format!(
                    "stage file '{}' appears to use awk interval-regex syntax on line {} in '{}'; BusyBox awk may reject that pattern",
                    stage_id,
                    line_number + 1,
                    resolved_path.display()
                ),
                Some(format!("stage:{stage_id}")),
            ));
        }
    }
    diagnostics
}
