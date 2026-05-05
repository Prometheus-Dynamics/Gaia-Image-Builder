use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawInputOptionConfig {
    pub description: Option<String>,
    pub kind: RawInputKind,
    pub required: bool,
    pub default: Option<String>,
    pub default_from: Option<RawInputDefaultFrom>,
    pub choices: Vec<String>,
    pub choices_from: Option<RawInputChoicesFromConfig>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputKind {
    #[default]
    String,
    Integer,
    Boolean,
    Enum,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputDefaultFrom {
    FirstChoice,
    LatestStable,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawInputChoicesFromConfig {
    pub kind: RawInputChoicesFromKind,
    pub repo: String,
    pub source: Option<String>,
    pub url: Option<String>,
    pub json_path: Option<String>,
    pub command: Vec<String>,
    pub pattern: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub strip_prefix: Option<String>,
    pub display_template: Option<String>,
    pub selected_value_template: Option<String>,
    pub limit: Option<usize>,
    pub sort: RawInputChoicesSort,
    pub prefer_stable: bool,
    pub refresh: RawInputChoicesRefresh,
    pub cache_ttl_seconds: Option<u64>,
    pub max_age_warning_seconds: Option<u64>,
    pub fallback_choices: Vec<String>,
    pub version_scheme: RawInputVersionScheme,
    pub allow_empty: bool,
    pub on_error: RawInputChoicesErrorMode,
    pub timeout_seconds: Option<u64>,
    pub auth_env: Option<String>,
    pub credential_helper: bool,
    pub include_prereleases: bool,
    pub include_drafts: bool,
    pub lock: bool,
    pub lock_key: Option<String>,
}

impl Default for RawInputChoicesFromConfig {
    fn default() -> Self {
        Self {
            kind: RawInputChoicesFromKind::GitTags,
            repo: String::new(),
            source: None,
            url: None,
            json_path: None,
            command: Vec::new(),
            pattern: None,
            include: Vec::new(),
            exclude: Vec::new(),
            strip_prefix: None,
            display_template: None,
            selected_value_template: None,
            limit: None,
            sort: RawInputChoicesSort::VersionDesc,
            prefer_stable: false,
            refresh: RawInputChoicesRefresh::Auto,
            cache_ttl_seconds: None,
            max_age_warning_seconds: None,
            fallback_choices: Vec::new(),
            version_scheme: RawInputVersionScheme::Versionish,
            allow_empty: false,
            on_error: RawInputChoicesErrorMode::Fail,
            timeout_seconds: None,
            auth_env: None,
            credential_helper: false,
            include_prereleases: true,
            include_drafts: false,
            lock: false,
            lock_key: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputChoicesFromKind {
    #[default]
    GitTags,
    GitBranches,
    GithubReleases,
    Json,
    Command,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputChoicesSort {
    LexicalAsc,
    LexicalDesc,
    VersionAsc,
    #[default]
    VersionDesc,
    PublishedDesc,
    PublishedAsc,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputVersionScheme {
    Semver,
    #[default]
    Versionish,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputChoicesRefresh {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RawInputChoicesErrorMode {
    #[default]
    Fail,
    Warn,
    Ignore,
}
