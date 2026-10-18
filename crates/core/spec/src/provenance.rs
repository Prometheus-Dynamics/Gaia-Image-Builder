#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProvenanceSpec {
    pub identity: ProvenanceIdentitySpec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProvenanceIdentitySpec {
    pub project: Option<String>,
    pub vendor: Option<String>,
    pub channel: Option<String>,
    pub labels: Vec<(String, String)>,
}
