#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SelectionSpec {
    pub requested_build: Option<String>,
    pub selected_build_file: Option<String>,
    pub selected_preset: Option<String>,
    pub selected_inputs: Vec<(String, String)>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    pub precedence_order: Vec<String>,
    /// Git sources whose files were imported into the config at resolve time.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub import_sources: Vec<ImportSourceSpec>,
    /// The variant this spec builds (`None` when the build declares none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Every variant the build declares, in file order (empty when none).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<crate::VariantSpec>,
}

/// A source whose checkout supplied config files through
/// `imports = [{ source = "<id>", path = "..." }]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImportSourceSpec {
    /// Source id, as declared in `[[sources]]`.
    pub id: String,
    /// Absolute directory the imported files were read from.
    pub root: String,
    /// Content identity: `git:<repo>@<commit>` for a checkout, or
    /// `path:<dir>#<digest of the imported config files>` for a
    /// `--set sources.<id>.path=<dir>` override.
    pub identity: String,
    /// Items declared by files imported from this source, as
    /// `<kind>:<id>` keys (`source`, `artifact`, `install`, `stage-file`,
    /// `stage-env-set`, `stage-service`) plus `image` when a file set any
    /// `[image]` field. Reuse folds `identity` into the fingerprints of
    /// the matching operations.
    pub contributes: Vec<String>,
}

impl ImportSourceSpec {
    pub fn contributes_to(&self, key: &str) -> bool {
        self.contributes.iter().any(|entry| entry == key)
    }
}
