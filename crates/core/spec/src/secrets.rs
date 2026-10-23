//! `[secrets]`: the entries a build file names for values it does not store,
//! and the `${env:NAME}` references that look like secrets.
//!
//! An entry names one source (`env`, `file` or `command`). The spec keeps the
//! source as written: the value is read when a process or registry request
//! uses it (see `gaia_process::secrets`), so it is never in the spec.

use std::path::Path;

use gaia_process::secrets::{SecretDef, SecretSource};

use crate::ResolvedBuildSpec;

/// Records the entries of `spec` with the process layer, so a reference to
/// one of them can be read when it is used, and sets the masking replacement.
/// Relative file paths are taken from the workspace root. Every path that
/// reads a secret calls this first, so a run, a registry cache and a cache
/// command all see the same entries.
pub fn install_secrets(spec: &ResolvedBuildSpec) {
    let base = Path::new(&spec.workspace.root_dir);
    let defs: Vec<SecretDef> = spec
        .secrets
        .entries
        .iter()
        .filter_map(|entry| {
            let source = if let Some(var) = &entry.env {
                SecretSource::Env(var.clone())
            } else if let Some(path) = &entry.file {
                SecretSource::File(gaia_process::secrets::file_path(path, base))
            } else if let Some(argv) = &entry.command {
                SecretSource::Command(argv.clone())
            } else {
                // Validation rejects an entry with no source before a run.
                return None;
            };
            Some(SecretDef {
                name: entry.name.clone(),
                source,
                required: entry.required,
            })
        })
        .collect();
    gaia_process::secrets::install(defs, &spec.reporting.masking.replacement);
}

/// The `[secrets]` table of a resolved build.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SecretsSpec {
    /// The entries, sorted by name.
    pub entries: Vec<SecretSpec>,
    /// Names of `${env:NAME}` references whose name looks like a secret. The
    /// value of each is in the spec, so validation warns about it.
    pub env_like: Vec<String>,
}

impl SecretsSpec {
    /// The entry called `name`.
    pub fn entry(&self, name: &str) -> Option<&SecretSpec> {
        self.entries.iter().find(|entry| entry.name == name)
    }
}

/// One `[secrets]` entry. Exactly one of `env`, `file` and `command` is set;
/// validation reports any other combination.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecretSpec {
    pub name: String,
    /// `env = "VAR"`: the value of an environment variable.
    pub env: Option<String>,
    /// `file = "path"`: the contents of a file, `~` expanded and a relative
    /// path taken from the workspace root. The file must be mode 0600 or
    /// stricter about group and others.
    pub file: Option<String>,
    /// `command = ["program", "arg", ...]`: the trimmed standard output.
    pub command: Option<Vec<String>>,
    /// Whether a missing or failed source is an error when the secret is used.
    pub required: bool,
}
