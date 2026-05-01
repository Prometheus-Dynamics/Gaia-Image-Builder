#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSpec {
    pub requested_build: Option<String>,
    pub selected_build_file: Option<String>,
    pub selected_preset: Option<String>,
    pub selected_inputs: Vec<(String, String)>,
    pub env_files: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    pub precedence_order: Vec<String>,
}
