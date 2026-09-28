use std::{env, fs, path::Path};

pub const EXAMPLE_DEFAULT_BUILD_CONFIG: &str = "examples/default-workspace/configs/default.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppArgs {
    pub command: AppCommand,
    pub build: String,
    /// True when the build config came from the command line rather than a default.
    pub build_explicit: bool,
    /// Directory the TUI build picker scans for build entrypoints.
    pub builds_dir: Option<String>,
    pub preset: Option<String>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    pub clean: CleanArgs,
    /// `--only` targets for run/plan: build domains or operation ids.
    pub only: Vec<String>,
    /// Problems found while parsing; dispatch refuses to run when non-empty.
    pub usage_errors: Vec<String>,
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
        let mut args = args.into_iter().map(Into::into).peekable();
        let mut parsed = Self {
            build: String::new(),
            ..Self::default()
        };

        let command = match args.peek().map(String::as_str) {
            Some("-h" | "--help" | "help") => Some(AppCommand::Help),
            Some("-V" | "--version" | "version") => Some(AppCommand::Version),
            Some("resolve") => Some(AppCommand::Resolve),
            Some("tui") => Some(AppCommand::Tui),
            Some("validate") => Some(AppCommand::Validate),
            Some("plan") => Some(AppCommand::Plan),
            Some("clean") => Some(AppCommand::Clean),
            Some("run") => Some(AppCommand::Run),
            _ => None,
        };
        if let Some(command) = command {
            args.next();
            parsed.command = command;
        }
        if matches!(parsed.command, AppCommand::Help | AppCommand::Version) {
            return parsed;
        }

        while let Some(arg) = args.next() {
            let mut value = |flag: &str, errors: &mut Vec<String>| {
                let value = args.next();
                if value.is_none() {
                    errors.push(format!("{flag} requires a value"));
                }
                value
            };
            match arg.as_str() {
                "-h" | "--help" => {
                    parsed.command = AppCommand::Help;
                    return parsed;
                }
                "--preset" => parsed.preset = value("--preset", &mut parsed.usage_errors),
                "--builds-dir" => {
                    parsed.builds_dir = value("--builds-dir", &mut parsed.usage_errors)
                }
                "--env-file" => {
                    if let Some(path) = value("--env-file", &mut parsed.usage_errors) {
                        parsed.env_files.push(path);
                    }
                }
                "--env" | "--set" => {
                    let Some(pair) = value(&arg, &mut parsed.usage_errors) else {
                        continue;
                    };
                    let Some((key, raw_value)) = pair.split_once('=') else {
                        parsed
                            .usage_errors
                            .push(format!("{arg} expects KEY=VALUE, got '{pair}'"));
                        continue;
                    };
                    let entry = (key.to_string(), raw_value.to_string());
                    if arg == "--env" {
                        parsed.env_overrides.push(entry);
                    } else {
                        parsed.explicit_overrides.push(entry);
                    }
                }
                "--profile" | "--clean-profile" => {
                    parsed.clean.profile = value(&arg, &mut parsed.usage_errors)
                }
                "--target" => {
                    if let Some(target) = value("--target", &mut parsed.usage_errors) {
                        parsed.clean.targets.push(target);
                    }
                }
                "--path" => {
                    if let Some(path) = value("--path", &mut parsed.usage_errors) {
                        parsed.clean.paths.push(path);
                    }
                }
                "--dry-run" => parsed.clean.dry_run = true,
                "--only" => {
                    if let Some(targets) = value("--only", &mut parsed.usage_errors) {
                        parsed.only.extend(
                            targets
                                .split(',')
                                .map(str::trim)
                                .filter(|target| !target.is_empty())
                                .map(str::to_string),
                        );
                    }
                }
                flag if flag.starts_with('-') => {
                    parsed.usage_errors.push(format!("unknown flag '{flag}'"));
                }
                positional if !parsed.build_explicit => {
                    parsed.build = positional.to_string();
                    parsed.build_explicit = true;
                }
                positional => parsed
                    .usage_errors
                    .push(format!("unexpected argument '{positional}'")),
            }
        }

        if !parsed.build_explicit {
            parsed.build = default_build_config();
        }
        parsed
    }
}

impl Default for AppArgs {
    fn default() -> Self {
        Self {
            command: AppCommand::Run,
            build: default_build_config(),
            build_explicit: false,
            builds_dir: None,
            preset: None,
            env_files: Vec::new(),
            env_overrides: Vec::new(),
            explicit_overrides: Vec::new(),
            clean: CleanArgs::default(),
            only: Vec::new(),
            usage_errors: Vec::new(),
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
