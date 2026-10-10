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
    pub cache: CacheArgs,
    pub lock: LockArgs,
    /// `--only` targets for run/plan: build domains or operation ids.
    pub only: Vec<String>,
    /// `--follow` / `-f` for status: refresh until the run ends.
    pub follow: bool,
    /// `--json` for preview: the report as JSON.
    pub json: bool,
    /// `--fail-on-clean` for preview: exit 3 when a run would clean or delete.
    pub fail_on_clean: bool,
    /// `--export <dir>` for run: copy the primary image output there after a
    /// successful run.
    pub export_dir: Option<String>,
    /// Problems found while parsing; dispatch refuses to run when non-empty.
    pub usage_errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanArgs {
    pub profile: Option<String>,
    pub targets: Vec<String>,
    pub paths: Vec<String>,
    pub dry_run: bool,
    /// `--all-caches`: with the `caches` target, also remove the shared
    /// git, download, Buildroot download and docker tool caches.
    pub all_caches: bool,
}

/// `gaia cache`: lists the package cache unless `remove` or `clear` is given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheArgs {
    /// `--level system|project`: only that level.
    pub level: Option<String>,
    /// `--package <glob>`: only matching packages.
    pub packages: Vec<String>,
    /// `--remove pkg[@key-prefix],...`: those entries.
    pub remove: Vec<String>,
    /// `--clear system|project|ccache`: a whole level, or the compiler cache.
    pub clear: Option<String>,
    /// `--remove-legacy`: entries of the first cache format (zstd tarballs).
    pub remove_legacy: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockArgs {
    /// `--update`: re-resolve locked sources instead of keeping them.
    pub update: bool,
    /// Source ids named after `--update`; empty means every git source.
    pub sources: Vec<String>,
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
            Some("preview") => Some(AppCommand::Preview),
            Some("clean") => Some(AppCommand::Clean),
            Some("cache") => Some(AppCommand::Cache),
            Some("pause") => Some(AppCommand::Pause),
            Some("resume") => Some(AppCommand::Resume),
            Some("cancel") => Some(AppCommand::Cancel),
            Some("status") => Some(AppCommand::Status),
            Some("lock") => Some(AppCommand::Lock),
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
                "--dry-run" => {
                    parsed.clean.dry_run = true;
                    parsed.cache.dry_run = true;
                }
                "--list" => {}
                "--remove-legacy" => parsed.cache.remove_legacy = true,
                "--level" => parsed.cache.level = value("--level", &mut parsed.usage_errors),
                "--clear" => parsed.cache.clear = value("--clear", &mut parsed.usage_errors),
                "--package" | "--remove" => {
                    if let Some(list) = value(&arg, &mut parsed.usage_errors) {
                        let list = split_list(&list);
                        if arg == "--package" {
                            parsed.cache.packages.extend(list);
                        } else {
                            parsed.cache.remove.extend(list);
                        }
                    }
                }
                "--all-caches" => parsed.clean.all_caches = true,
                "--follow" | "-f" => parsed.follow = true,
                "--json" => parsed.json = true,
                "--fail-on-clean" => parsed.fail_on_clean = true,
                "--export" => parsed.export_dir = value("--export", &mut parsed.usage_errors),
                "--update" => {
                    parsed.lock.update = true;
                    // `--update [source-id]`: an optional value, taken only
                    // once the build path is known so it is never mistaken
                    // for the build.
                    if parsed.build_explicit
                        && let Some(next) = args.next_if(|next| !next.starts_with('-'))
                    {
                        parsed.lock.sources.extend(split_list(&next));
                    }
                }
                update if update.starts_with("--update=") => {
                    parsed.lock.update = true;
                    parsed
                        .lock
                        .sources
                        .extend(split_list(&update["--update=".len()..]));
                }
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
        // `gaia run --dry-run` is `gaia preview`.
        if parsed.command == AppCommand::Run && parsed.clean.dry_run {
            parsed.command = AppCommand::Preview;
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
            cache: CacheArgs::default(),
            lock: LockArgs::default(),
            only: Vec::new(),
            follow: false,
            json: false,
            fail_on_clean: false,
            export_dir: None,
            usage_errors: Vec::new(),
        }
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
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
    Cache,
    /// Signal the running `gaia run` of a build.
    Pause,
    Resume,
    Cancel,
    /// Show what the running `gaia run` of a build is doing.
    Status,
    Lock,
    Run,
    /// What `gaia run` would do, without changing anything.
    Preview,
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

    fn parse(args: &[&str]) -> AppArgs {
        AppArgs::parse_from(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn preview_takes_the_plan_options_and_its_own_flags() {
        let args = parse(&[
            "preview",
            "examples/default-workspace/configs/default.toml",
            "--set",
            "image.defconfig=x",
            "--only",
            "image",
            "--json",
            "--fail-on-clean",
        ]);
        assert_eq!(args.command, AppCommand::Preview);
        assert!(args.json && args.fail_on_clean);
        assert_eq!(args.only, ["image"]);
        assert_eq!(
            args.explicit_overrides,
            [("image.defconfig".to_string(), "x".to_string())]
        );
        assert!(args.usage_errors.is_empty(), "{:?}", args.usage_errors);
    }

    #[test]
    fn run_with_dry_run_is_preview_and_clean_keeps_its_own_dry_run() {
        let run = parse(&["run", "build.toml", "--dry-run"]);
        assert_eq!(run.command, AppCommand::Preview);
        assert!(run.clean.dry_run);
        let clean = parse(&["clean", "build.toml", "--dry-run"]);
        assert_eq!(clean.command, AppCommand::Clean);
        assert!(clean.clean.dry_run);
        assert_eq!(parse(&["run", "build.toml"]).command, AppCommand::Run);
        assert_eq!(
            parse(&["--dry-run", "build.toml"]).command,
            AppCommand::Preview
        );
    }
}
