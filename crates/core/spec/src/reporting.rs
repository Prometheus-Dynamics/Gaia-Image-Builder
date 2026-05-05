#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportingSpec {
    pub outputs: ReportingOutputsSpec,
    pub masking: SecretMaskingSpec,
    pub output_hygiene: OutputHygieneSpec,
    pub post_build: Option<PostBuildHookSpec>,
}

pub const DEFAULT_LARGE_UNEXPECTED_OUTPUT_BYTES: u64 = 100 * 1024 * 1024;
pub const DEFAULT_TRANSIENT_DIR_NAMES: &[&str] =
    &[".cache", "build", "buildroot-output", "sources", "target"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputHygieneSpec {
    pub large_file_threshold_bytes: u64,
    pub transient_dir_names: Vec<String>,
}

impl Default for OutputHygieneSpec {
    fn default() -> Self {
        Self {
            large_file_threshold_bytes: DEFAULT_LARGE_UNEXPECTED_OUTPUT_BYTES,
            transient_dir_names: DEFAULT_TRANSIENT_DIR_NAMES
                .iter()
                .map(|name| (*name).into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingOutputsSpec {
    pub summary: bool,
    pub provenance: bool,
    pub manifest: bool,
}

impl Default for ReportingOutputsSpec {
    fn default() -> Self {
        Self {
            summary: true,
            provenance: true,
            manifest: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMaskingSpec {
    pub enabled: bool,
    pub replacement: String,
    pub patterns: Vec<String>,
}

impl Default for SecretMaskingSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            replacement: "***".into(),
            patterns: vec![
                "TOKEN".into(),
                "SECRET".into(),
                "PASSWORD".into(),
                "PRIVATE_KEY".into(),
                "API_KEY".into(),
                "ACCESS_KEY".into(),
                "CREDENTIAL".into(),
                "AUTH".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostBuildHookSpec {
    pub script: String,
    pub timeout_seconds: u64,
}
