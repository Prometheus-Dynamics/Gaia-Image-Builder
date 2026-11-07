//! A trimmed Rust library (`trim = { users = [...] }`): Gaia relinks it to
//! export only what its users import, so the users must be built by the same
//! cargo invocation (its build group) and the library must be one `.so`.

use gaia_spec::{ArtifactDefinition, ArtifactVariantSpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_rust_trims(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for artifact in &spec.artifacts {
        let ArtifactDefinition::Rust(rust) = &artifact.definition else {
            continue;
        };
        // Members that only record the group's links carry no users.
        let Some(trim) = rust.trim.as_ref().filter(|trim| !trim.users.is_empty()) else {
            continue;
        };
        let id = artifact.id.as_str();
        let location = Some(format!("artifact:{id}"));
        let mut report = |code: &'static str, message: String| {
            diagnostics.push(error(code, message, location.clone()));
        };
        let file_name = rust.target_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&artifact.output.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        if rust.is_std_library()
            || rust.variant != ArtifactVariantSpec::File
            || !file_name.ends_with(".so")
        {
            report(
                "rust_trim_not_a_library",
                format!(
                    "rust artifact '{id}' sets trim, which relinks one shared library (a `.so` file, not emit_directory or package = \"std\"); its file is '{file_name}'"
                ),
            );
        }
        let Some(group) = rust.build_group.as_deref() else {
            report(
                "rust_trim_without_build_group",
                format!(
                    "rust artifact '{id}' sets trim: its users must be built by the same cargo invocation, so it and each user need one build_group"
                ),
            );
            continue;
        };
        for user in &trim.users {
            let member = spec
                .artifacts
                .iter()
                .find(|other| other.id.as_str() == user);
            let in_group = member.is_some_and(|other| {
                matches!(&other.definition, ArtifactDefinition::Rust(other_rust)
                    if other_rust.build_group.as_deref() == Some(group) && !other_rust.is_std_library())
            });
            if user == id || !in_group {
                report(
                    "rust_trim_user_not_in_group",
                    format!(
                        "rust artifact '{id}' trims for user '{user}', which is not another crate-building member of its build_group '{group}'"
                    ),
                );
            }
        }
    }
}
