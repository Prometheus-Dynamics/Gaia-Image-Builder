use std::env;

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
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "tui" => Self {
                command: AppCommand::Tui,
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "validate" => Self {
                command: AppCommand::Validate,
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "plan" => Self {
                command: AppCommand::Plan,
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "clean" => Self {
                command: AppCommand::Clean,
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
                preset: None,
                env_files: Vec::new(),
                env_overrides: Vec::new(),
                explicit_overrides: Vec::new(),
                clean: CleanArgs::default(),
            },
            "run" => Self {
                command: AppCommand::Run,
                build: args
                    .next()
                    .unwrap_or_else(|| "examples/default-workspace/configs/default.toml".into()),
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
            build: "examples/default-workspace/configs/default.toml".into(),
            preset: None,
            env_files: Vec::new(),
            env_overrides: Vec::new(),
            explicit_overrides: Vec::new(),
            clean: CleanArgs::default(),
        }
    }
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
