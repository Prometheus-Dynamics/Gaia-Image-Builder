use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

pub(crate) fn validate_reporting(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if !spec.reporting.outputs.summary
        && !spec.reporting.outputs.provenance
        && !spec.reporting.outputs.manifest
    {
        diagnostics.push(warning(
            "reporting_outputs_disabled",
            "all reporting outputs are disabled for this build".into(),
            Some("reporting".into()),
        ));
    }
}

/// Keys this Gaia does not know. They are errors: a file using a setting
/// from a newer Gaia must fail rather than build without it.
pub(crate) fn validate_config_warnings(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for message in &spec.metadata.config_warnings {
        diagnostics.push(error(
            "config_unknown_key",
            format!("{message} {}", newer_gaia_hint()),
            None,
        ));
    }
    for message in &spec.metadata.empty_layers {
        diagnostics.push(warning("config_layer_empty", message.clone(), None));
    }
    let expected = &spec.metadata.expected;
    let missing = |kind: &str, ids: &[String], present: &dyn Fn(&str) -> bool| {
        ids.iter()
            .filter(|id| !present(id))
            .map(|id| {
                format!(
                    "[expect] {kind} '{id}' is not in the build; a layer that should provide \
                     it is missing, empty or no longer imported"
                )
            })
            .collect::<Vec<_>>()
    };
    let messages = [
        missing("artifact", &expected.artifacts, &|id| {
            spec.artifacts
                .iter()
                .any(|artifact| artifact.id.as_str() == id)
        }),
        missing("install", &expected.installs, &|id| {
            spec.install
                .entries
                .iter()
                .any(|install| install.id.as_str() == id)
        }),
        missing("source", &expected.sources, &|id| {
            spec.sources.iter().any(|source| source.id.as_str() == id)
        }),
    ];
    for message in messages.into_iter().flatten() {
        diagnostics.push(error(
            "config_expected_missing",
            message,
            Some("expect".into()),
        ));
    }
}

/// An unknown key is most often a setting of a newer Gaia than the one
/// running: say which one runs, and how a build file asks for a newer one.
fn newer_gaia_hint() -> String {
    format!(
        "(this is gaia {}; if the key comes from a newer Gaia, set gaia_version = \">=<that version>\" in the file so an older one says so first)",
        env!("CARGO_PKG_VERSION")
    )
}
