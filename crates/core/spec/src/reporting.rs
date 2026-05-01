#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportingSpec {
    pub outputs: ReportingOutputsSpec,
    pub masking: SecretMaskingSpec,
    pub post_build: Option<PostBuildHookSpec>,
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
