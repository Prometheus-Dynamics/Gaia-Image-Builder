#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildMetadataSpec {
    pub version: Option<String>,
    pub description: Option<String>,
    pub branch: Option<String>,
    pub target: Option<String>,
    pub profile: Option<String>,
    pub labels: Vec<(String, String)>,
    pub product: ProductIdentitySpec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductIdentitySpec {
    pub family: Option<String>,
    pub name: Option<String>,
    pub sku: Option<String>,
}
