#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildMetadataSpec {
    pub version: Option<String>,
    pub description: Option<String>,
    pub branch: Option<String>,
    pub target: Option<String>,
    pub profile: Option<String>,
    pub labels: Vec<(String, String)>,
    pub product: ProductIdentitySpec,
    /// Problems found while loading the build files (for example unknown
    /// keys), reported as validation warnings.
    pub config_warnings: Vec<String>,
    /// Loaded config files that set nothing (only comments), reported as
    /// validation warnings.
    pub empty_layers: Vec<String>,
    /// `[expect]`: ids the build must have.
    pub expected: ExpectedItemsSpec,
}

/// Ids a build declares it must end up with (`[expect]`); validation fails
/// when one is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpectedItemsSpec {
    pub artifacts: Vec<String>,
    pub installs: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductIdentitySpec {
    pub family: Option<String>,
    pub name: Option<String>,
    pub sku: Option<String>,
}
