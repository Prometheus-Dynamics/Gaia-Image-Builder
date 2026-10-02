#[derive(Clone, Default, PartialEq, Eq)]
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
    pub import_sources: Vec<ImportSourceSpec>,
}

// Fingerprints hash the Debug output of the spec. `import_sources` is only
// printed when non-empty so builds without source imports keep the
// fingerprints they had before source imports existed.
impl std::fmt::Debug for SelectionSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("SelectionSpec");
        debug
            .field("requested_build", &self.requested_build)
            .field("selected_build_file", &self.selected_build_file)
            .field("selected_preset", &self.selected_preset)
            .field("selected_inputs", &self.selected_inputs)
            .field("env_files", &self.env_files)
            .field("env_overrides", &self.env_overrides)
            .field("explicit_overrides", &self.explicit_overrides)
            .field("precedence_order", &self.precedence_order);
        if !self.import_sources.is_empty() {
            debug.field("import_sources", &self.import_sources);
        }
        debug.finish()
    }
}

/// A source whose checkout supplied config files through
/// `imports = [{ source = "<id>", path = "..." }]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
