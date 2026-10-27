//! The environment cargo runs with besides Gaia's toolchain and cache
//! settings: the artifact's `build_env` and the offline switch.
use gaia_artifact_providers::{ArtifactProviderError, ArtifactProviderErrorKind};

/// The artifact's `build_env` with each `${secret:NAME}` read now, so the
/// value reaches only cargo's environment (the docker backend passes it as
/// `-e KEY` with the value in its own environment, never on the command line).
pub(crate) fn cargo_build_env(
    build_env: &[(String, String)],
) -> Result<Vec<(String, String)>, ArtifactProviderError> {
    build_env
        .iter()
        .map(|(key, value)| {
            gaia_process::secrets::expand_child_value(value)
                .map(|value| (key.clone(), value))
                .map_err(|error| {
                    ArtifactProviderError::new(
                        ArtifactProviderErrorKind::PolicyBlocked,
                        format!("build_env '{key}': {error}"),
                    )
                })
        })
        .collect()
}

/// The environment an offline run gives cargo: it uses only the registry and
/// git caches that are already there, and never contacts a registry.
pub(crate) fn offline_cargo_env(offline: bool) -> Vec<(&'static str, &'static str)> {
    if offline {
        vec![("CARGO_NET_OFFLINE", "true")]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_values_pass_through_and_an_undefined_secret_names_the_key() {
        assert_eq!(
            cargo_build_env(&[("CARGO_TERM_COLOR".into(), "never".into())]).expect("env"),
            [("CARGO_TERM_COLOR".to_string(), "never".to_string())]
        );
        let error = cargo_build_env(&[(
            "GIT_TOKEN".into(),
            "${secret:gaia_rust_build_env_undefined}".into(),
        )])
        .expect_err("undefined secret");
        assert!(
            error.message.starts_with("build_env 'GIT_TOKEN':"),
            "{}",
            error.message
        );
    }
}
