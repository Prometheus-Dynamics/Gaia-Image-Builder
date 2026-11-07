//! What a rust artifact asks of its cargo invocation besides the packages.

use gaia_spec::{ArtifactDefinition, ArtifactSpec, CargoGitSourceSpec};
use std::process::Command;

/// Cargo feature selection of a rust artifact, and the git sources its cargo
/// git dependencies come from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CargoFeatureFlags {
    pub(crate) features: Vec<String>,
    pub(crate) no_default_features: bool,
    pub(crate) all_features: bool,
    /// `cargo_git_sources`: prepared on the host before cargo runs. Not in
    /// the state fields: the artifact's fingerprint covers them.
    pub(crate) git_sources: Vec<CargoGitSourceSpec>,
    /// `build_env`: in the fingerprint, not the state fields, like the above.
    pub(crate) build_env: Vec<(String, String)>,
    /// The `[cpu]` profile's flags: in the fingerprint, like the above.
    pub(crate) cpu: Option<gaia_spec::RustCpuSpec>,
    /// `link`: shared adds `-C prefer-dynamic` to the target's rustc flags.
    pub(crate) link: gaia_spec::RustLinkSpec,
    /// The group's trimmed libraries: their links go through Gaia's wrapper
    /// (`link_record`). In the fingerprint, like the above.
    pub(crate) recorded: Vec<String>,
}

impl CargoFeatureFlags {
    pub(crate) fn of(artifact: &ArtifactSpec) -> Self {
        match &artifact.definition {
            ArtifactDefinition::Rust(rust) => Self {
                features: rust.features.clone(),
                no_default_features: rust.no_default_features,
                all_features: rust.all_features,
                git_sources: rust.cargo_git_sources.clone(),
                build_env: rust.build_env.clone(),
                cpu: rust.cpu.clone(),
                link: rust.link,
                recorded: rust
                    .trim
                    .as_ref()
                    .map(|trim| trim.recorded.clone())
                    .unwrap_or_default(),
            },
            _ => Self::default(),
        }
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        if self.all_features {
            command.arg("--all-features");
        }
        if self.no_default_features {
            command.arg("--no-default-features");
        }
        if !self.features.is_empty() {
            command.arg("--features").arg(self.features.join(","));
        }
    }

    /// State fields, only for non-default selections so existing artifact
    /// state (and therefore reuse) is unchanged for artifacts without flags.
    pub(crate) fn state_fields(&self) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        if !self.features.is_empty() {
            fields.push(("features".to_string(), self.features.join(",")));
        }
        if self.no_default_features {
            fields.push(("no_default_features".to_string(), "true".to_string()));
        }
        if self.all_features {
            fields.push(("all_features".to_string(), "true".to_string()));
        }
        if !self.link.is_static() {
            fields.push(("link".to_string(), "shared".to_string()));
        }
        fields
    }
}
