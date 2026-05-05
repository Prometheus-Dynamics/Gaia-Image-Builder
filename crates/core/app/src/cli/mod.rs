use std::{env, fs, path::Path};

pub const EXAMPLE_DEFAULT_BUILD_CONFIG: &str = "examples/default-workspace/configs/default.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppArgs {
    pub command: AppCommand,
    pub build: String,
    pub preset: Option<String>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    pub clean: CleanArgs,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanArgs {
    pub profile: Option<String>,
    pub targets: Vec<String>,
    pub paths: Vec<String>,
    pub dry_run: bool,
}

impl AppArgs {
    pub fn from_env() -> Self {
        Self::parse_from(env::args().skip(1))
    }

    pub fn parse_from<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut args = args.into_iter().map(Into::into);
        let Some(first) = args.next() else {
            return Self::default();
        };

        let mut parsed = match first.as_str() {
            "-h" | "--help" | "help" => Self {
                command: AppCommand::Help,
                build: String::new(),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "-V" | "--version" | "version" => Self {
                command: AppCommand::Version,
                build: String::new(),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "resolve" => Self {
                command: AppCommand::Resolve,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "tui" => Self {
                command: AppCommand::Tui,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "validate" => Self {
                command: AppCommand::Validate,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "plan" => Self {
                command: AppCommand::Plan,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "clean" => Self {
                command: AppCommand::Clean,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "run" => Self {
                command: AppCommand::Run,
                build: args.next().unwrap_or_else(default_build_config),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            build => Self {
                command: AppCommand::Run,
                build: build.into(),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
        };

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--preset" => {
                    parsed.preset = args.next();
                }
                "--env-file" => {
                    if let Some(value) = args.next() {
                        parsed.env_files.push(value);
                    }
                }
                "--env" => {
                    if let Some(value) = args.next()
                        && let Some((key, raw_value)) = value.split_once('=')
                    {
                        parsed
                            .env_overrides
                            .push((key.to_string(), raw_value.to_string()));
                    }
                }
                "--set" => {
                    if let Some(value) = args.next()
                        && let Some((key, raw_value)) = value.split_once('=')
                    {
                        parsed
                            .explicit_overrides
                            .push((key.to_string(), raw_value.to_string()));
                    }
                }
                "--profile" | "--clean-profile" => {
                    parsed.clean.profile = args.next();
                }
                "--target" => {
                    if let Some(value) = args.next() {
                        parsed.clean.targets.push(value);
                    }
                }
                "--path" => {
                    if let Some(value) = args.next() {
                        parsed.clean.paths.push(value);
                    }
                }
                "--dry-run" => {
                    parsed.clean.dry_run = true;
                }
                _ => {}
            }
        }

        parsed
    }
}

impl Default for AppArgs {
    fn default() -> Self {
        Self {
            command: AppCommand::Run,
            build: default_build_config(),
            preset: None,
            env_files: Vec::new(),
            env_overrides: Vec::new(),
            explicit_overrides: Vec::new(),
            clean: CleanArgs::default(),
        }
    }
}

fn default_build_config() -> String {
    default_build_config_in_dir(Path::new("."))
}

fn default_build_config_in_dir(dir: &Path) -> String {
    let build_toml = dir.join("build.toml");
    if build_toml.is_file() {
        return build_toml.display().to_string();
    }

    let mut toml_paths = current_dir_build_toml_files(dir);
    toml_paths.sort();
    toml_paths
        .into_iter()
        .next()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| EXAMPLE_DEFAULT_BUILD_CONFIG.into())
}

pub(crate) fn current_dir_build_toml_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .filter(|path| {
            !matches!(
                path.file_name().and_then(|value| value.to_str()),
                Some("Cargo.toml" | "rust-toolchain.toml")
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    Help,
    Version,
    Resolve,
    Tui,
    Validate,
    Plan,
    Clean,
    Run,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!("gaia-app-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn default_build_config_prefers_current_dir_build_toml() {
        let dir = temp_dir("build-toml");
        fs::write(dir.join("build.toml"), "build_name = \"local\"\n").expect("build toml");
        fs::write(dir.join("other.toml"), "build_name = \"other\"\n").expect("other toml");

        assert_eq!(
            default_build_config_in_dir(&dir),
            dir.join("build.toml").display().to_string()
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn default_build_config_uses_current_dir_toml_before_example_default() {
        let dir = temp_dir("single-toml");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"not-a-build\"\n",
        )
        .expect("cargo toml");
        fs::write(dir.join("local.toml"), "build_name = \"local\"\n").expect("local toml");

        assert_eq!(
            default_build_config_in_dir(&dir),
            dir.join("local.toml").display().to_string()
        );

        let _ = fs::remove_dir_all(dir);
    }
}
