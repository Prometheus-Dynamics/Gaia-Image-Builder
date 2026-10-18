#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct InputSpec {
    pub declared: Vec<InputOptionSpec>,
    pub selected: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InputOptionSpec {
    pub name: String,
    pub description: Option<String>,
    pub kind: InputKindSpec,
    pub required: bool,
    pub default: Option<String>,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum InputKindSpec {
    #[default]
    String,
    Integer,
    Boolean,
    Enum,
}
