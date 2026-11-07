//! `kind = "command"` artifacts: a host command run in its source's
//! directory, whose output is collected from that directory or written to
//! `GAIA_ARTIFACT_OUTPUT`.

use std::path::{Component, Path};

use gaia_spec::{ArtifactExecutionSpec, ArtifactSpec, CommandArtifactSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_command_artifact(
    artifact: &ArtifactSpec,
    command: &CommandArtifactSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let id = artifact.id.as_str();
    let location = Some(format!("artifact:{id}"));
    let mut push = |code: &'static str, message: String| {
        diagnostics.push(error(code, message, location.clone()));
    };
    if command.command.is_empty() {
        push(
            "command_artifact_command_empty",
            format!("command artifact '{id}' has no `command` (the program and its arguments)"),
        );
    } else if command.command.iter().any(|arg| arg.trim().is_empty()) {
        push(
            "command_artifact_command_empty_arg",
            format!("command artifact '{id}' has an empty entry in `command`"),
        );
    }
    if artifact.source.is_none() {
        push(
            "command_artifact_source_missing",
            format!(
                "command artifact '{id}' names no `source`: the command runs in its source's \
                 directory"
            ),
        );
    }
    if let Some(ArtifactExecutionSpec::Docker(_)) = &artifact.execution {
        push(
            "command_artifact_docker_unsupported",
            format!(
                "command artifact '{id}' selects docker execution: command artifacts run on \
                 the host only"
            ),
        );
    }
    if command
        .build_env
        .iter()
        .any(|(key, _)| key.trim().is_empty())
    {
        push(
            "command_artifact_build_env_key_empty",
            format!("command artifact '{id}' has a `build_env` entry with an empty name"),
        );
    }
    if let Some(collect) = &command.collect
        && !is_inside_relative(collect)
    {
        push(
            "command_artifact_collect_outside",
            format!(
                "command artifact '{id}' collects '{collect}': `collect` must be a path inside \
                 the source directory (relative, without `..`)"
            ),
        );
    }
    if command.collect.is_none() && !command.collect_exclude.is_empty() {
        push(
            "command_artifact_exclude_without_collect",
            format!(
                "command artifact '{id}' sets `collect_exclude` without `collect`: the excludes \
                 apply to the collected directory"
            ),
        );
    }
    for exclude in &command.collect_exclude {
        if !is_inside_relative(exclude) {
            push(
                "command_artifact_exclude_outside",
                format!(
                    "command artifact '{id}' excludes '{exclude}': `collect_exclude` entries \
                     are paths inside `collect` (relative, without `..`)"
                ),
            );
        }
    }
}

/// A non-empty relative path with no `..` or root component.
fn is_inside_relative(path: &str) -> bool {
    !path.trim().is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::{
        ArtifactDefinition, ArtifactOutputSpec, DockerArtifactExecutionSpec, SourceRef,
    };

    fn artifact(command: CommandArtifactSpec) -> ArtifactSpec {
        ArtifactSpec::new(
            "ui",
            ArtifactDefinition::Command(command),
            Some(SourceRef::new("ui")),
            ArtifactOutputSpec {
                path: "out/ui".into(),
            },
        )
    }

    fn codes(artifact: &ArtifactSpec) -> Vec<&'static str> {
        let ArtifactDefinition::Command(command) = &artifact.definition else {
            unreachable!()
        };
        let mut diagnostics = Vec::new();
        validate_command_artifact(artifact, command, &mut diagnostics);
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    fn spec(command: &[&str]) -> CommandArtifactSpec {
        CommandArtifactSpec {
            command: command.iter().map(|arg| arg.to_string()).collect(),
            build_env: Vec::new(),
            collect: Some("build".into()),
            collect_exclude: vec!["mock".into()],
        }
    }

    #[test]
    fn a_complete_command_artifact_passes() {
        assert!(codes(&artifact(spec(&["sh", "-c", "make"]))).is_empty());
    }

    #[test]
    fn empty_commands_and_entries_are_refused() {
        assert_eq!(
            codes(&artifact(spec(&[]))),
            ["command_artifact_command_empty"]
        );
        assert_eq!(
            codes(&artifact(spec(&["sh", " "]))),
            ["command_artifact_command_empty_arg"]
        );
    }

    #[test]
    fn docker_execution_and_a_missing_source_are_refused() {
        let mut docker = artifact(spec(&["make"]));
        docker.execution = Some(ArtifactExecutionSpec::Docker(
            DockerArtifactExecutionSpec::default(),
        ));
        assert_eq!(codes(&docker), ["command_artifact_docker_unsupported"]);
        let mut sourceless = artifact(spec(&["make"]));
        sourceless.source = None;
        assert_eq!(codes(&sourceless), ["command_artifact_source_missing"]);
    }

    #[test]
    fn collect_paths_stay_inside_the_source() {
        let mut outside = spec(&["make"]);
        outside.collect = Some("../elsewhere".into());
        outside.collect_exclude = vec!["/abs".into()];
        assert_eq!(
            codes(&artifact(outside)),
            [
                "command_artifact_collect_outside",
                "command_artifact_exclude_outside"
            ]
        );
        let mut no_collect = spec(&["make"]);
        no_collect.collect = None;
        assert_eq!(
            codes(&artifact(no_collect)),
            ["command_artifact_exclude_without_collect"]
        );
    }
}
