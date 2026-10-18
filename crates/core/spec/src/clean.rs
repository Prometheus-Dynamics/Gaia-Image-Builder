use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CleanSpec {
    pub default_profile: Option<String>,
    pub profiles: BTreeMap<String, CleanProfileSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CleanProfileSpec {
    pub description: Option<String>,
    pub build: bool,
    pub out: bool,
    pub paths: Vec<String>,
}
