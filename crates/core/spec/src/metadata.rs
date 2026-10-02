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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductIdentitySpec {
    pub family: Option<String>,
    pub name: Option<String>,
    pub sku: Option<String>,
}
