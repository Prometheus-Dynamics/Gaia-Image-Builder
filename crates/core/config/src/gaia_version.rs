//! `gaia_version = ">=2.1.0"`: the Gaia versions a build file works with.
//!
//! Checked on the raw TOML of every loaded file (including imports and
//! `extends`) before anything else reads it, so an older binary stops with
//! an upgrade hint instead of silently ignoring keys it does not know.
use std::path::Path;

use crate::ConfigError;

/// The version of this Gaia binary.
pub const GAIA_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const UPGRADE_COMMAND: &str =
    "cargo install --git https://github.com/Prometheus-Dynamics/Gaia-Image-Builder gaia";

pub(crate) fn check_required_gaia_version(
    path: &Path,
    value: &toml::Value,
) -> Result<(), ConfigError> {
    check_required_gaia_version_against(path, value, GAIA_VERSION)
}

pub(crate) fn check_required_gaia_version_against(
    path: &Path,
    value: &toml::Value,
    installed: &str,
) -> Result<(), ConfigError> {
    let Some(requirement) = value.get("gaia_version") else {
        return Ok(());
    };
    let Some(requirement) = requirement.as_str() else {
        return Err(ConfigError::config_shape(
            path,
            "gaia_version must be a version requirement string, for example \">=2.1.0\"",
        ));
    };
    let parsed = semver::VersionReq::parse(requirement.trim()).map_err(|error| {
        ConfigError::config_shape(
            path,
            format!(
                "gaia_version '{requirement}' is not a valid version requirement \
                 (for example \">=2.1.0\"): {error}"
            ),
        )
    })?;
    let installed_version = semver::Version::parse(installed).map_err(|error| {
        ConfigError::config_shape(
            path,
            format!("installed gaia version '{installed}' is not semver: {error}"),
        )
    })?;
    if parsed.matches(&installed_version) {
        return Ok(());
    }
    Err(ConfigError::GaiaVersionUnsupported {
        path: path.display().to_string(),
        required: requirement.trim().to_string(),
        installed: installed.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> toml::Value {
        toml::from_str(text).expect("toml")
    }

    #[test]
    fn missing_requirement_is_accepted() {
        let value = config("build_name = \"demo\"\n");
        assert!(check_required_gaia_version_against(Path::new("b.toml"), &value, "2.0.0").is_ok());
    }

    #[test]
    fn matching_requirement_is_accepted() {
        let value = config("gaia_version = \">=2.1.0\"\n");
        assert!(check_required_gaia_version_against(Path::new("b.toml"), &value, "2.1.0").is_ok());
        assert!(check_required_gaia_version_against(Path::new("b.toml"), &value, "3.0.0").is_ok());
    }

    #[test]
    fn older_binary_fails_with_upgrade_hint() {
        let value = config("gaia_version = \">=2.1.0\"\nbuild_command = \"x\"\n");
        let error = check_required_gaia_version_against(Path::new("b.toml"), &value, "2.0.0")
            .expect_err("2.0.0 is too old");
        assert_eq!(
            error,
            ConfigError::GaiaVersionUnsupported {
                path: "b.toml".into(),
                required: ">=2.1.0".into(),
                installed: "2.0.0".into(),
            }
        );
        let message = error.to_string();
        assert!(
            message.starts_with("this build requires gaia >=2.1.0, but gaia 2.0.0 is installed")
        );
        assert!(message.contains("upgrade with: cargo install"));
    }

    #[test]
    fn invalid_requirement_is_a_shape_error() {
        for text in ["gaia_version = \"latest\"\n", "gaia_version = 2\n"] {
            let error =
                check_required_gaia_version_against(Path::new("b.toml"), &config(text), "2.1.0")
                    .expect_err("invalid requirement");
            assert!(matches!(error, ConfigError::ConfigShape { .. }), "{error}");
        }
    }

    #[test]
    fn this_binary_satisfies_its_own_version() {
        let value = config(&format!("gaia_version = \"={GAIA_VERSION}\"\n"));
        assert!(check_required_gaia_version(Path::new("b.toml"), &value).is_ok());
    }
}
