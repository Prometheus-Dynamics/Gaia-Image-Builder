#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub label: String,
}

impl ProcessSpec {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}
