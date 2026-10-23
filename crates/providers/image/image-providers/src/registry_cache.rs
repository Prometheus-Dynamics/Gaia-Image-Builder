//! The registry cache: the `[[checkpoints]]` entry with `backend = "oci"`
//! read as a cache of build products (Buildroot packages, artifact outputs,
//! image layers) in an OCI registry. The checkpoint's anchor is not used:
//! the registry holds entries of every kind, keyed by their input keys.
//!
//! `use_policy` allows pulls (a miss is looked up in the registry before it
//! is built), `upload_policy` allows pushes (what a run builds is pushed
//! after it), and pushes need a sign key. Pulls need a verify key unless
//! `trust = "unsigned"`; validation rejects a configuration that cannot
//! pull, so a key that is set in the environment but missing here is a
//! warning, not a failure of the build.

use gaia_process::{RegistryTarget, RemoteCache, SignatureKind};
use gaia_spec::{CheckpointBackendRef, CheckpointPolicy, ResolvedBuildSpec};

/// The registry cache of a run, when a checkpoint uses `backend = "oci"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCacheUse {
    /// The checkpoint that configured it.
    pub checkpoint_id: String,
    pub target: RegistryTarget,
    /// Misses are pulled.
    pub pull: bool,
    /// Built entries are pushed (needs a sign key in `target`).
    pub push: bool,
    /// Settings that could not be used (for example a sign key variable that
    /// is not set); the run reports them and goes on without them.
    pub warnings: Vec<String>,
}

impl RegistryCacheUse {
    /// A client for the registry. Credentials are resolved on first use.
    pub fn client(&self) -> RemoteCache {
        RemoteCache::new(self.target.clone())
    }
}

/// The registry cache the spec configures, or `None` when no checkpoint uses
/// `backend = "oci"` with a policy other than `off`. An error names a setting
/// that makes the registry unusable (an unknown signature kind, or a verify
/// key variable that is not set while pulls are on).
pub fn registry_cache_from_spec(
    spec: &ResolvedBuildSpec,
) -> Result<Option<RegistryCacheUse>, String> {
    let Some(point) = spec.checkpoints.points.iter().find(|point| {
        point
            .backend
            .as_ref()
            .is_some_and(|backend| backend.backend == "oci")
    }) else {
        return Ok(None);
    };
    let pull = point.use_policy != CheckpointPolicy::Off;
    let push = point.upload_policy != CheckpointPolicy::Off;
    if !pull && !push {
        return Ok(None);
    }
    let id = point.id.as_str().to_string();
    let oci = point
        .backend
        .as_ref()
        .and_then(|backend: &CheckpointBackendRef| backend.oci.as_ref())
        .ok_or_else(|| format!("checkpoint '{id}': backend oci has no registry settings"))?;
    let mut warnings = Vec::new();
    let verify_key = resolve_setting(oci.verify_key.as_deref())
        .map_err(|error| format!("checkpoint '{id}' verify_key: {error}"))?;
    let sign_key = match resolve_setting(oci.sign_key.as_deref()) {
        Ok(key) => key,
        Err(error) => {
            warnings.push(format!(
                "checkpoint '{id}' sign_key: {error}; nothing is pushed"
            ));
            None
        }
    };
    let kind_text = oci
        .sign_kind
        .clone()
        .or_else(|| spec.image.signing.as_ref().map(|signing| signing.kind.clone()))
        .unwrap_or_else(|| "minisign".to_string());
    let kind = SignatureKind::parse(&kind_text).ok_or_else(|| {
        format!("checkpoint '{id}' sign_kind '{kind_text}' is not minisign, gpg or cosign-blob")
    })?;
    if pull && verify_key.is_none() && !oci.trust_unsigned() {
        return Err(format!(
            "checkpoint '{id}' pulls from the registry but has no verify_key; pulls are refused"
        ));
    }
    let target = RegistryTarget {
        registry: oci.registry.clone(),
        repository: oci.repository.clone(),
        kind,
        sign_key: sign_key.clone(),
        verify_key,
        trust_unsigned: oci.trust_unsigned(),
        tool_dir: None,
    };
    Ok(Some(RegistryCacheUse {
        checkpoint_id: id,
        target,
        pull,
        push: push && sign_key.is_some(),
        warnings,
    }))
}

/// A key setting: `env:VAR` names an environment variable that holds the key;
/// anything else is the key (or a path to it). `None` when the setting is not
/// given. The error names the variable, never its value.
pub fn resolve_setting(setting: Option<&str>) -> Result<Option<String>, String> {
    let Some(setting) = setting.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let Some(name) = setting.strip_prefix("env:") else {
        return Ok(Some(setting.to_string()));
    };
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_string())),
        _ => Err(format!("environment variable '{name}' is not set or empty")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_setting_is_the_key_and_env_names_a_variable() {
        assert_eq!(resolve_setting(None), Ok(None));
        assert_eq!(resolve_setting(Some("  ")), Ok(None));
        assert_eq!(
            resolve_setting(Some("/keys/gaia.pub")),
            Ok(Some("/keys/gaia.pub".to_string()))
        );
        let error = resolve_setting(Some("env:GAIA_TEST_REGISTRY_KEY_UNSET_ZZ")).unwrap_err();
        assert!(error.contains("GAIA_TEST_REGISTRY_KEY_UNSET_ZZ"), "{error}");
    }
}
