//! Rust `cargo_git_sources`: each cargo git dependency maps to a declared
//! git source that supplies its locked commits. An id that names no source
//! is reported by the artifact's extra sources; this checks the kind and
//! that two keys cannot name the same repository for different sources.

use std::collections::BTreeMap;

use gaia_spec::{ArtifactDefinition, ResolvedBuildSpec, SourceDefinition, cargo_git_url_key};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_cargo_git_sources(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for artifact in &spec.artifacts {
        let ArtifactDefinition::Rust(rust) = &artifact.definition else {
            continue;
        };
        let location = Some(format!("artifact:{}", artifact.id.as_str()));
        let mut keys = BTreeMap::<String, &str>::new();
        for mapping in &rust.cargo_git_sources {
            let source_id = mapping.source.as_str();
            if mapping.url.trim().is_empty() {
                diagnostics.push(error(
                    "rust_cargo_git_url_empty",
                    format!(
                        "rust artifact '{}' has an empty URL in cargo_git_sources (for source '{source_id}')",
                        artifact.id.as_str()
                    ),
                    location.clone(),
                ));
                continue;
            }
            if let Some(source) = spec
                .sources
                .iter()
                .find(|source| source.id.as_str() == source_id)
                && !matches!(source.definition, SourceDefinition::Git(_))
            {
                diagnostics.push(error(
                    "rust_cargo_git_source_not_git",
                    format!(
                        "rust artifact '{}' maps cargo git dependency '{}' to source '{source_id}', which is a {} source; cargo_git_sources needs a kind = \"git\" source",
                        artifact.id.as_str(),
                        mapping.url,
                        source.definition.provider_kind().as_str()
                    ),
                    location.clone(),
                ));
            }
            let key = cargo_git_url_key(&mapping.url);
            match keys.get(&key) {
                Some(other) if *other != source_id => diagnostics.push(error(
                    "rust_cargo_git_source_conflict",
                    format!(
                        "rust artifact '{}' maps the same cargo git repository '{key}' to both '{other}' and '{source_id}'",
                        artifact.id.as_str()
                    ),
                    location.clone(),
                )),
                Some(_) => {}
                None => {
                    keys.insert(key, source_id);
                }
            }
        }
    }
}
