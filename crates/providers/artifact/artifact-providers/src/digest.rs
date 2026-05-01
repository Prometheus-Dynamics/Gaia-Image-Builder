use crate::{ArtifactExecutionContract, render_build_context_state};
use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;

pub fn command_version_line<S: AsRef<OsStr>>(program: S, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output();
    let Ok(output) = output else {
        return "unavailable".to_string();
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(line) = stdout.lines().find(|line| !line.trim().is_empty()) {
        return line.trim().to_string();
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(line) = stderr.lines().find(|line| !line.trim().is_empty()) {
        return line.trim().to_string();
    }

    "unavailable".to_string()
}

pub fn produced_filename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

pub struct ArtifactBackendState<'a> {
    pub contract: &'a ArtifactExecutionContract,
    pub provider_id: &'a str,
    pub artifact_id: &'a str,
    pub resolved_identifier_kind: &'a str,
    pub resolved_identifier: &'a str,
    pub output_class: &'a str,
    pub build_tool: &'a str,
    pub build_tool_version: &'a str,
    pub extra_fields: &'a [(String, String)],
}

pub fn render_artifact_backend_state(state: ArtifactBackendState<'_>) -> String {
    let output_path = Path::new(&state.contract.output.path);
    let mut rendered = gaia_spec::KeyValueState::new()
        .with("provider", state.provider_id)
        .with("artifact", state.artifact_id)
        .with("resolved_identifier_kind", state.resolved_identifier_kind)
        .with("resolved_identifier", state.resolved_identifier)
        .with("produced_filename", produced_filename(output_path))
        .with("output_class", state.output_class)
        .with("build_tool", state.build_tool)
        .with("build_tool_version", state.build_tool_version)
        .with("output", state.contract.output.path.as_str())
        .with("output_sha256", file_sha256_or_placeholder(output_path))
        .with("output_bytes", path_bytes(output_path));
    rendered.extend_pairs(state.extra_fields.iter().cloned());
    let mut rendered = rendered.render();
    rendered.push_str(&render_build_context_state(state.contract));
    rendered
}

pub fn file_sha256_or_placeholder(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().ok();
    let Some(output) = output else {
        return format!("sha256-unavailable:{}", path.display());
    };
    if !output.status.success() {
        return format!(
            "sha256-error:{}:{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

pub fn path_bytes(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub fn dir_digest(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    hash_dir(path, &mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_dir(path: &Path, hasher: &mut DefaultHasher) {
    path.display().to_string().hash(hasher);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            "missing".hash(hasher);
            return;
        }
    };
    metadata.is_dir().hash(hasher);
    metadata.is_file().hash(hasher);
    metadata.len().hash(hasher);
    if metadata.is_dir() {
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort();
        for entry in entries {
            hash_dir(&entry, hasher);
        }
    } else {
        file_sha256_or_placeholder(path).hash(hasher);
    }
}
