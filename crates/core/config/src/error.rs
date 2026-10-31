//! The errors resolving a build config returns.

use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    CurrentDir {
        message: String,
    },
    ConfigNotFound {
        build: String,
        searched: Vec<String>,
    },
    ConfigPath {
        path: String,
        message: String,
    },
    /// An `imports` or `extends` entry names a file that does not exist.
    ConfigReferenceMissing {
        referenced_by: String,
        field: &'static str,
        reference: String,
        resolved: String,
    },
    ConfigRead {
        path: String,
        message: String,
    },
    ConfigParse {
        path: String,
        message: String,
    },
    ConfigShape {
        path: String,
        message: String,
    },
    /// A patch entry (see `raw::RawEntryPatch`) that names no earlier entry,
    /// sets no field, or does not make a valid entry. The message names the
    /// patch and its `<layer>:<line>`.
    ConfigPatch {
        message: String,
    },
    /// `gaia migrate` could not check or keep a migration; the files are as
    /// they were.
    ConfigMigrate {
        message: String,
    },
    ConfigImportCycle {
        cycle: Vec<String>,
    },
    EnvFileRead {
        path: String,
        message: String,
    },
    MissingPreset {
        preset: String,
    },
    InvalidOverrideValue {
        key: String,
        value: String,
        expected: &'static str,
    },
    /// A `--set` (or preset) key that names no setting, or an `input.<name>`
    /// for an input the build does not declare. Such a key would otherwise
    /// be silently ignored.
    InvalidOverrideKey {
        key: String,
        message: String,
    },
    /// `${...}` references that lead back to a token being resolved, such as
    /// `build.name -> build.display_name -> build.name`.
    InterpolationCycle {
        cycle: Vec<String>,
    },
    /// `sources.<id>.path` names a source that no config the build selects
    /// declares. `scope` names what was searched, such as "the selected
    /// config" or "any selected variant".
    UndeclaredSourceOverride {
        key: String,
        source_id: String,
        scope: String,
        declared: Vec<String>,
    },
    /// The build file's `gaia_version` requirement does not match this
    /// binary.
    GaiaVersionUnsupported {
        path: String,
        required: String,
        installed: String,
    },
    /// An `imports` entry with `source = "<id>"` (or an `@source:<id>`
    /// reference) could not be resolved.
    ImportSource {
        source_id: String,
        referenced_by: String,
        message: String,
    },
}

impl ConfigError {
    pub(crate) fn current_dir(error: impl fmt::Display) -> Self {
        Self::CurrentDir {
            message: error.to_string(),
        }
    }

    pub(crate) fn config_path(path: &Path, error: impl fmt::Display) -> Self {
        Self::ConfigPath {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn config_read(path: &Path, error: impl fmt::Display) -> Self {
        Self::ConfigRead {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    /// A parse or type error of a build file's `text`, located at toml's span
    /// as `path:line:col`. Without a span the error names the file only.
    pub(crate) fn config_parse_at(path: &Path, text: &str, error: &toml::de::Error) -> Self {
        let span = error.span();
        let message = if span.is_some() {
            error.message().to_string()
        } else {
            error.to_string()
        };
        Self::ConfigParse {
            path: crate::locate::toml_error_location(path, text, span),
            message,
        }
    }

    pub(crate) fn config_shape(path: &Path, message: impl Into<String>) -> Self {
        Self::ConfigShape {
            path: path.display().to_string(),
            message: message.into(),
        }
    }

    pub(crate) fn env_file_read(path: &Path, error: impl fmt::Display) -> Self {
        Self::EnvFileRead {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn invalid_override_value(
        key: impl Into<String>,
        value: impl Into<String>,
        expected: &'static str,
    ) -> Self {
        Self::InvalidOverrideValue {
            key: key.into(),
            value: value.into(),
            expected,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDir { message } => {
                write!(
                    formatter,
                    "current working directory must be readable: {message}"
                )
            }
            Self::ConfigNotFound { build, searched } => write!(
                formatter,
                "failed to locate build config for '{build}', looked in: {}",
                searched.join(", ")
            ),
            Self::ConfigPath { path, message } => {
                write!(formatter, "failed to canonicalize '{path}': {message}")
            }
            Self::ConfigReferenceMissing {
                referenced_by,
                field,
                reference,
                resolved,
            } => write!(
                formatter,
                "{field} '{reference}' in '{referenced_by}' does not exist (resolved to '{resolved}')"
            ),
            Self::ConfigRead { path, message } => {
                write!(formatter, "failed to read build config '{path}': {message}")
            }
            Self::ConfigParse { path, message } => {
                write!(
                    formatter,
                    "failed to parse build config '{path}': {message}"
                )
            }
            Self::ConfigShape { path, message } => {
                write!(
                    formatter,
                    "failed to parse build config '{path}': {message}"
                )
            }
            Self::ConfigPatch { message } => write!(formatter, "{message}"),
            Self::ConfigMigrate { message } => write!(formatter, "gaia migrate: {message}"),
            Self::ConfigImportCycle { cycle } => {
                write!(
                    formatter,
                    "config import cycle detected: {}",
                    cycle.join(" -> ")
                )
            }
            Self::EnvFileRead { path, message } => {
                write!(formatter, "failed to read env file '{path}': {message}")
            }
            Self::MissingPreset { preset } => write!(
                formatter,
                "selected preset '{preset}' was not defined in the resolved config"
            ),
            Self::InvalidOverrideValue {
                key,
                value,
                expected,
            } => write!(
                formatter,
                "invalid override value for '{key}': '{value}' (expected {expected})"
            ),
            Self::InvalidOverrideKey { key, message } => {
                write!(formatter, "invalid override key '{key}': {message}")
            }
            Self::InterpolationCycle { cycle } => {
                write!(formatter, "interpolation cycle: {}", cycle.join(" -> "))
            }
            Self::UndeclaredSourceOverride {
                key,
                source_id,
                scope,
                declared,
            } => {
                let declared = if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.join(", ")
                };
                write!(
                    formatter,
                    "override '{key}' names source '{source_id}', which is not declared by {scope} (declared sources: {declared})"
                )
            }
            Self::GaiaVersionUnsupported {
                path,
                required,
                installed,
            } => write!(
                formatter,
                "this build requires gaia {required}, but gaia {installed} is installed \
                 (gaia_version in '{path}'); upgrade with: {}",
                crate::gaia_version::UPGRADE_COMMAND
            ),
            Self::ImportSource {
                source_id,
                referenced_by,
                message,
            } => write!(
                formatter,
                "import source '{source_id}' used by '{referenced_by}': {message}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}
