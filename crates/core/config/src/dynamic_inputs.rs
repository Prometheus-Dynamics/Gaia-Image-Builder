use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use crate::ConfigError;
use crate::overrides::collect_selected_inputs;
use crate::raw::{
    RawBuildConfig, RawInputChoicesErrorMode, RawInputChoicesFromConfig, RawInputChoicesFromKind,
    RawInputChoicesRefresh, RawInputChoicesSort, RawInputDefaultFrom, RawInputVersionScheme,
    RawSourceDefinition,
};

#[path = "dynamic_inputs_glob.rs"]
mod dynamic_inputs_glob;
#[path = "dynamic_inputs_identity.rs"]
mod dynamic_inputs_identity;
#[cfg(test)]
#[path = "dynamic_inputs_tests.rs"]
mod dynamic_inputs_tests;

use dynamic_inputs_glob::glob_matches;
use dynamic_inputs_identity::dynamic_choices_identity_key;

const DYNAMIC_INPUT_TIMEOUT_SECONDS: u64 = 30;

pub(crate) fn resolve_dynamic_inputs(
    mut raw: RawBuildConfig,
) -> Result<RawBuildConfig, ConfigError> {
    let source_path = raw
        .source_path
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    let source_dir = raw
        .source_path
        .as_deref()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let sources = raw.sources.clone();

    for (name, input) in raw.inputs.iter_mut() {
        let Some(choices_from) = input.choices_from.clone() else {
            continue;
        };
        let choices =
            resolve_choices_from(&choices_from, &sources, &source_dir).map_err(|message| {
                ConfigError::config_shape(
                    std::path::Path::new(&source_path),
                    format!("failed to resolve choices for input '{name}': {message}"),
                )
            })?;
        input.choices = merge_choices(std::mem::take(&mut input.choices), choices);
        if input.default.is_none() {
            input.default = match input.default_from {
                Some(RawInputDefaultFrom::FirstChoice) => input.choices.first().cloned(),
                Some(RawInputDefaultFrom::LatestStable) => input
                    .choices
                    .iter()
                    .find(|choice| is_stable_choice(choice))
                    .or_else(|| input.choices.first())
                    .cloned(),
                None => None,
            };
        }
    }

    raw.selected_inputs = collect_selected_inputs(&raw);
    Ok(raw)
}

fn resolve_choices_from(
    config: &RawInputChoicesFromConfig,
    sources: &[crate::raw::RawSourceConfig],
    source_dir: &Path,
) -> Result<Vec<String>, String> {
    let repo = resolve_repo(config, sources)?;
    if repo.trim().is_empty()
        && matches!(
            config.kind,
            RawInputChoicesFromKind::GitTags
                | RawInputChoicesFromKind::GitBranches
                | RawInputChoicesFromKind::GithubReleases
        )
    {
        return Err("choices_from.repo cannot be empty".to_string());
    }

    let cache_path = cache_path(config, &repo, source_dir);
    let lock_path = lock_path(config, &repo, source_dir);
    if config.lock
        && let Some(locked) = read_cache(&lock_path)
    {
        return Ok(locked);
    }
    match config.refresh {
        RawInputChoicesRefresh::Never => {
            return fallback_choices(config, &cache_path, "dynamic choices refresh is disabled");
        }
        RawInputChoicesRefresh::Auto => {
            if let Some(ttl) = config.cache_ttl_seconds
                && let Some(cached) = read_cache_if_fresh(&cache_path, ttl)
            {
                warn_if_cache_old(config, &cache_path);
                return Ok(cached);
            }
        }
        RawInputChoicesRefresh::Always => {}
    }

    match fetch_choices(config, &repo) {
        Ok(choices) => {
            let choices = normalize_choices(config, choices);
            if !choices.is_empty() || config.allow_empty {
                write_cache(&cache_path, &choices);
                if config.lock {
                    write_cache(&lock_path, &choices);
                }
                return Ok(choices);
            }
            handle_choice_error(
                config,
                fallback_choices(
                    config,
                    &cache_path,
                    "dynamic choice source returned no choices",
                ),
            )
        }
        Err(error) => handle_choice_error(config, fallback_choices(config, &cache_path, &error)),
    }
}

fn fetch_choices(config: &RawInputChoicesFromConfig, repo: &str) -> Result<Vec<String>, String> {
    match config.kind {
        RawInputChoicesFromKind::GitTags | RawInputChoicesFromKind::GitBranches => {
            fetch_git_choices(config, repo)
        }
        RawInputChoicesFromKind::GithubReleases => fetch_github_releases(config, repo),
        RawInputChoicesFromKind::Json => fetch_json_choices(config),
        RawInputChoicesFromKind::Command => fetch_command_choices(config),
    }
}

fn handle_choice_error(
    config: &RawInputChoicesFromConfig,
    result: Result<Vec<String>, String>,
) -> Result<Vec<String>, String> {
    match result {
        Ok(choices) => Ok(choices),
        Err(error) => match config.on_error {
            RawInputChoicesErrorMode::Fail => Err(error),
            RawInputChoicesErrorMode::Warn => {
                tracing::warn!(error, "dynamic input choices unavailable");
                Ok(Vec::new())
            }
            RawInputChoicesErrorMode::Ignore => Ok(Vec::new()),
        },
    }
}

fn warn_if_cache_old(config: &RawInputChoicesFromConfig, path: &Path) {
    let Some(max_age) = config.max_age_warning_seconds else {
        return;
    };
    if let Ok(metadata) = fs::metadata(path)
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = SystemTime::now().duration_since(modified)
        && age > Duration::from_secs(max_age)
    {
        tracing::warn!(
            cache = %path.display(),
            age_seconds = age.as_secs(),
            max_age,
            "dynamic input choices cache is older than warning threshold"
        );
    }
}

fn fetch_github_releases(
    config: &RawInputChoicesFromConfig,
    repo: &str,
) -> Result<Vec<String>, String> {
    let url = if repo.starts_with("http://") || repo.starts_with("https://") {
        repo.to_string()
    } else {
        format!("https://api.github.com/repos/{repo}/releases")
    };
    let body = fetch_url(config, &url)?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|error| format!("invalid GitHub releases JSON: {error}"))?;
    let releases = json
        .as_array()
        .ok_or_else(|| "GitHub releases response was not an array".to_string())?;
    Ok(releases
        .iter()
        .filter(|release| {
            config.include_drafts
                || !release
                    .get("draft")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
        })
        .filter(|release| {
            config.include_prereleases
                || !release
                    .get("prerelease")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
        })
        .filter_map(|release| release.get("tag_name").and_then(|value| value.as_str()))
        .map(ToString::to_string)
        .collect())
}

fn fetch_json_choices(config: &RawInputChoicesFromConfig) -> Result<Vec<String>, String> {
    let source = config
        .url
        .as_deref()
        .or_else(|| (!config.repo.trim().is_empty()).then_some(config.repo.as_str()))
        .ok_or_else(|| "choices_from.url is required for json choices".to_string())?;
    let body = if source.starts_with("http://") || source.starts_with("https://") {
        fetch_url(config, source)?
    } else {
        fs::read_to_string(source)
            .map_err(|error| format!("failed to read JSON choices: {error}"))?
    };
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| format!("invalid JSON choices: {error}"))?;
    let path = config.json_path.as_deref().unwrap_or("$");
    Ok(json_path_values(&json, path))
}

fn fetch_command_choices(config: &RawInputChoicesFromConfig) -> Result<Vec<String>, String> {
    let Some((program, args)) = config.command.split_first() else {
        return Err("choices_from.command cannot be empty".to_string());
    };
    let mut command = Command::new(program);
    command.args(args);
    let output = run_dynamic_input_command(config, command, "dynamic input choices command")?;
    if !output.status.success() {
        return Err(format!(
            "choices command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn fetch_url(config: &RawInputChoicesFromConfig, url: &str) -> Result<String, String> {
    fetch_url_with_program(config, url, Path::new("curl"))
}

fn fetch_url_with_program(
    config: &RawInputChoicesFromConfig,
    url: &str,
    curl_program: &Path,
) -> Result<String, String> {
    fetch_url_with_auth_resolver(config, url, curl_program, |name| std::env::var(name).ok())
}

fn fetch_url_with_auth_resolver(
    config: &RawInputChoicesFromConfig,
    url: &str,
    curl_program: &Path,
    auth_resolver: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let mut command = Command::new(curl_program);
    command
        .arg("-fsSL")
        .arg("-H")
        .arg("User-Agent: gaia-config");
    if let Some(timeout) = config.timeout_seconds {
        command.arg("--max-time").arg(timeout.to_string());
    }
    if let Some(auth_env) = config.auth_env.as_deref()
        && let Some(token) = auth_resolver(auth_env)
        && !token.trim().is_empty()
    {
        let auth_config = write_curl_auth_config(&token)?;
        command.arg("--config").arg(&auth_config);
        command.arg(url);
        let output = run_dynamic_input_command(config, command, "dynamic input curl fetch");
        let _ = fs::remove_file(auth_config);
        let output = output?;
        if !output.status.success() {
            return Err(format!(
                "curl failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        return String::from_utf8(output.stdout)
            .map_err(|error| format!("response was not UTF-8: {error}"));
    }
    command.arg(url);
    let output = run_dynamic_input_command(config, command, "dynamic input curl fetch")?;
    if !output.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("response was not UTF-8: {error}"))
}

fn run_dynamic_input_command(
    config: &RawInputChoicesFromConfig,
    mut command: Command,
    label: &str,
) -> Result<Output, String> {
    let timeout = Duration::from_secs(
        config
            .timeout_seconds
            .unwrap_or(DYNAMIC_INPUT_TIMEOUT_SECONDS)
            .max(1),
    );
    gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        timeout,
        label,
        gaia_process::ProcessOutputRetention::default(),
        None,
        None,
    )
    .map(|result| result.output)
    .map_err(|error| error.message)
}

fn write_curl_auth_config(token: &str) -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir()
        .join("gaia-dynamic-input")
        .join(format!("curl-auth-{}-{nonce}.config", std::process::id()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create curl auth config dir: {error}"))?;
    }
    fs::write(
        &path,
        format!(
            "header = \"Authorization: Bearer {}\"\n",
            curl_config_string(token)?
        ),
    )
    .map_err(|error| format!("failed to write curl auth config: {error}"))?;
    restrict_auth_file_permissions(&path)?;
    Ok(path)
}

fn curl_config_string(value: &str) -> Result<String, String> {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' | '\r' => {
                return Err("curl auth token cannot contain newline characters".to_string());
            }
            _ => escaped.push(ch),
        }
    }
    Ok(escaped)
}

#[cfg(unix)]
fn restrict_auth_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to inspect curl auth config permissions: {error}"))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to restrict curl auth config permissions: {error}"))
}

#[cfg(not(unix))]
fn restrict_auth_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn json_path_values(json: &serde_json::Value, path: &str) -> Vec<String> {
    if path == "$" {
        return json_value_strings(json);
    }
    let normalized = path.trim_start_matches("$.").trim_start_matches('$');
    if normalized.is_empty() {
        return json_value_strings(json);
    }
    let parts = normalized.split('.').collect::<Vec<_>>();
    collect_json_path(json, &parts)
}

fn collect_json_path(value: &serde_json::Value, parts: &[&str]) -> Vec<String> {
    if parts.is_empty() {
        return json_value_strings(value);
    }
    let part = parts[0];
    if let Some(array_field) = part.strip_suffix("[*]") {
        return value
            .get(array_field)
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .flat_map(|item| collect_json_path(item, &parts[1..]))
            .collect();
    }
    if part == "[*]" {
        return value
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|item| collect_json_path(item, &parts[1..]))
            .collect();
    }
    value
        .get(part)
        .map(|value| collect_json_path(value, &parts[1..]))
        .unwrap_or_default()
}

fn json_value_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) => vec![value.clone()],
        serde_json::Value::Array(values) => values.iter().flat_map(json_value_strings).collect(),
        _ => Vec::new(),
    }
}

fn resolve_repo(
    config: &RawInputChoicesFromConfig,
    sources: &[crate::raw::RawSourceConfig],
) -> Result<String, String> {
    if !config.repo.trim().is_empty() {
        return Ok(config.repo.clone());
    }
    let Some(source_id) = config.source.as_deref() else {
        return Ok(String::new());
    };
    let source = sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| format!("choices_from.source '{source_id}' was not declared"))?;
    match &source.definition {
        RawSourceDefinition::Git { repo, .. } => Ok(repo.clone()),
        _ => Err(format!(
            "choices_from.source '{source_id}' must reference a git source"
        )),
    }
}

fn fetch_git_choices(
    config: &RawInputChoicesFromConfig,
    repo: &str,
) -> Result<Vec<String>, String> {
    fetch_git_choices_with_program(config, repo, Path::new("git"))
}

fn fetch_git_choices_with_program(
    config: &RawInputChoicesFromConfig,
    repo: &str,
    git_program: &Path,
) -> Result<Vec<String>, String> {
    let mut command = Command::new(git_program);
    command.arg("ls-remote");
    match config.kind {
        RawInputChoicesFromKind::GitTags => {
            command.arg("--tags").arg("--refs");
        }
        RawInputChoicesFromKind::GitBranches => {
            command.arg("--heads");
        }
        _ => {}
    }
    command.arg(repo);
    let patterns = git_patterns(config);
    for pattern in &patterns {
        command.arg(pattern);
    }

    let output = run_dynamic_input_command(config, command, "dynamic input git ls-remote")?;
    if !output.status.success() {
        return Err(format!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let prefix = match config.kind {
        RawInputChoicesFromKind::GitTags => "refs/tags/",
        RawInputChoicesFromKind::GitBranches => "refs/heads/",
        _ => "",
    };
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix(prefix))
        .filter(|reference| !reference.ends_with("^{}"))
        .map(ToString::to_string)
        .collect::<Vec<_>>())
}

fn git_patterns(config: &RawInputChoicesFromConfig) -> Vec<String> {
    let mut patterns = Vec::new();
    if let Some(pattern) = config.pattern.as_deref()
        && !pattern.trim().is_empty()
    {
        patterns.push(pattern.to_string());
    }
    patterns.extend(
        config
            .include
            .iter()
            .filter(|pattern| !pattern.trim().is_empty())
            .cloned(),
    );
    patterns
}

fn normalize_choices(config: &RawInputChoicesFromConfig, choices: Vec<String>) -> Vec<String> {
    let mut choices = choices
        .into_iter()
        .filter(|choice| matches_filters(config, choice))
        .map(|choice| display_choice(config, choice))
        .map(|choice| selected_choice(config, choice))
        .collect::<Vec<_>>();

    sort_choices(
        &mut choices,
        config.sort,
        config.version_scheme,
        config.prefer_stable,
    );
    choices.dedup();
    if let Some(limit) = config.limit {
        choices.truncate(limit);
    }
    choices
}

fn display_choice(config: &RawInputChoicesFromConfig, choice: String) -> String {
    let choice = if let Some(prefix) = config.strip_prefix.as_deref() {
        choice.strip_prefix(prefix).unwrap_or(&choice).to_string()
    } else {
        choice
    };
    config
        .display_template
        .as_deref()
        .map(|template| template.replace("${choice}", &choice))
        .unwrap_or(choice)
}

fn selected_choice(config: &RawInputChoicesFromConfig, choice: String) -> String {
    config
        .selected_value_template
        .as_deref()
        .map(|template| template.replace("${choice}", &choice))
        .unwrap_or(choice)
}

fn matches_filters(config: &RawInputChoicesFromConfig, choice: &str) -> bool {
    let include_matches = config.include.is_empty()
        || config
            .include
            .iter()
            .any(|pattern| glob_matches(pattern, choice));
    let exclude_matches = config
        .exclude
        .iter()
        .any(|pattern| glob_matches(pattern, choice));
    include_matches && !exclude_matches
}

fn fallback_choices(
    config: &RawInputChoicesFromConfig,
    cache_path: &Path,
    error: &str,
) -> Result<Vec<String>, String> {
    if let Some(cached) = read_cache(cache_path) {
        return Ok(cached);
    }
    if !config.fallback_choices.is_empty() {
        return Ok(normalize_choices(config, config.fallback_choices.clone()));
    }
    Err(error.to_string())
}

fn cache_path(config: &RawInputChoicesFromConfig, repo: &str, source_dir: &Path) -> PathBuf {
    let key = dynamic_choices_identity_key(config, repo);
    source_dir
        .join(".gaia")
        .join("input-cache")
        .join(format!("{key}.choices"))
}

fn lock_path(config: &RawInputChoicesFromConfig, repo: &str, source_dir: &Path) -> PathBuf {
    let name = config
        .lock_key
        .clone()
        .unwrap_or_else(|| dynamic_choices_identity_key(config, repo));
    source_dir
        .join(".gaia")
        .join("input-lock")
        .join(format!("{name}.choices"))
}

fn read_cache_if_fresh(path: &Path, ttl_seconds: u64) -> Option<Vec<String>> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age <= Duration::from_secs(ttl_seconds) {
        read_cache(path)
    } else {
        None
    }
}

fn read_cache(path: &Path) -> Option<Vec<String>> {
    let contents = fs::read_to_string(path).ok()?;
    let choices = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    (!choices.is_empty()).then_some(choices)
}

fn write_cache(path: &Path, choices: &[String]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, choices.join("\n"));
}

fn merge_choices(mut static_choices: Vec<String>, dynamic_choices: Vec<String>) -> Vec<String> {
    for choice in dynamic_choices {
        if !static_choices.iter().any(|existing| existing == &choice) {
            static_choices.push(choice);
        }
    }
    static_choices
}

fn sort_choices(
    choices: &mut [String],
    sort: RawInputChoicesSort,
    version_scheme: RawInputVersionScheme,
    prefer_stable: bool,
) {
    choices.sort_by(|left, right| {
        if prefer_stable {
            let stable_order = is_stable_choice(right).cmp(&is_stable_choice(left));
            if stable_order != Ordering::Equal {
                return stable_order;
            }
        }
        match sort {
            RawInputChoicesSort::LexicalAsc => left.cmp(right),
            RawInputChoicesSort::LexicalDesc => right.cmp(left),
            RawInputChoicesSort::VersionAsc => version_cmp(left, right, version_scheme),
            RawInputChoicesSort::VersionDesc => version_cmp(right, left, version_scheme),
            RawInputChoicesSort::PublishedAsc => version_cmp(left, right, version_scheme),
            RawInputChoicesSort::PublishedDesc => version_cmp(right, left, version_scheme),
        }
    });
}

fn version_cmp(left: &str, right: &str, scheme: RawInputVersionScheme) -> Ordering {
    match scheme {
        RawInputVersionScheme::Semver => semver_cmp(left, right),
        RawInputVersionScheme::Versionish => versionish_cmp(left, right),
    }
}

fn semver_cmp(left: &str, right: &str) -> Ordering {
    let left = parse_semver(left);
    let right = parse_semver(right);
    left.cmp(&right)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SemverParts {
    major: u64,
    minor: u64,
    patch: u64,
    stable: bool,
    prerelease: String,
}

fn parse_semver(value: &str) -> SemverParts {
    let value = value.trim_start_matches('v');
    let (core, prerelease) = value.split_once('-').unwrap_or((value, ""));
    let mut parts = core.split('.');
    SemverParts {
        major: parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
        minor: parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
        patch: parts.next().and_then(|part| part.parse().ok()).unwrap_or(0),
        stable: prerelease.is_empty(),
        prerelease: prerelease.to_ascii_lowercase(),
    }
}

fn is_stable_choice(choice: &str) -> bool {
    let lower = choice.to_ascii_lowercase();
    !lower.contains('-')
        && !lower.contains("alpha")
        && !lower.contains("beta")
        && !lower.contains("rc")
        && !lower.contains("pre")
        && !lower.contains("snapshot")
}

fn versionish_cmp(left: &str, right: &str) -> Ordering {
    let mut left_parts = versionish_parts(left);
    let mut right_parts = versionish_parts(right);
    let len = left_parts.len().max(right_parts.len());
    left_parts.resize(len, VersionishPart::Text(String::new()));
    right_parts.resize(len, VersionishPart::Text(String::new()));
    for (left, right) in left_parts.iter().zip(right_parts.iter()) {
        let ordering = left.cmp(right);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionishPart {
    Number(u64),
    Text(String),
}

impl Ord for VersionishPart {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left.cmp(right),
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            (Self::Number(_), Self::Text(_)) => Ordering::Greater,
            (Self::Text(_), Self::Number(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for VersionishPart {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn versionish_parts(value: &str) -> Vec<VersionishPart> {
    let mut parts = Vec::new();
    let mut buffer = String::new();
    let mut numeric = None;
    for ch in value.chars() {
        let is_digit = ch.is_ascii_digit();
        if numeric == Some(is_digit) {
            buffer.push(ch);
            continue;
        }
        if !buffer.is_empty() {
            parts.push(parse_versionish_part(&buffer, numeric.unwrap_or(false)));
            buffer.clear();
        }
        numeric = Some(is_digit);
        buffer.push(ch);
    }
    if !buffer.is_empty() {
        parts.push(parse_versionish_part(&buffer, numeric.unwrap_or(false)));
    }
    parts
}

fn parse_versionish_part(value: &str, numeric: bool) -> VersionishPart {
    if numeric {
        VersionishPart::Number(value.parse::<u64>().unwrap_or(u64::MAX))
    } else {
        VersionishPart::Text(value.to_ascii_lowercase())
    }
}
