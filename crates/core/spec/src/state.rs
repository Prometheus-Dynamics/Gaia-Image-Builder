use std::collections::BTreeMap;

pub const RUNTIME_STATE_DIR_NAME: &str = ".gaia/runtime";
pub const IMAGE_ASSEMBLY_STATE_FILE_NAME: &str = "image-assembly.state";
pub const IMAGE_ASSEMBLY_STATE_KIND: &str = "image-assembly";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyValueState {
    fields: Vec<(String, String)>,
}

impl KeyValueState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.insert(key, value);
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl ToString) {
        self.fields
            .push((key.into(), sanitize_state_value(value.to_string())));
    }

    pub fn extend_pairs<K, V>(&mut self, pairs: impl IntoIterator<Item = (K, V)>)
    where
        K: Into<String>,
        V: ToString,
    {
        for (key, value) in pairs {
            self.insert(key, value);
        }
    }

    pub fn render(&self) -> String {
        let mut rendered = String::new();
        for (key, value) in &self.fields {
            rendered.push_str(key);
            rendered.push('=');
            rendered.push_str(value);
            rendered.push('\n');
        }
        rendered
    }

    pub fn parse(contents: &str) -> Self {
        let mut state = Self::new();
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            state.fields.push((key.to_string(), value.to_string()));
        }
        state
    }

    pub fn into_map(self) -> BTreeMap<String, String> {
        self.fields.into_iter().collect()
    }
}

fn sanitize_state_value(value: String) -> String {
    value.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_value_state_renders_and_parses_fields() {
        let state = KeyValueState::new()
            .with("kind", "install")
            .with("path", "/usr/bin/demo")
            .with("message", "line one\nline two");

        assert_eq!(
            state.render(),
            "kind=install\npath=/usr/bin/demo\nmessage=line one line two\n"
        );

        let parsed = KeyValueState::parse("kind=install\nbad-line\npath=/usr/bin/demo\n");
        let map = parsed.into_map();
        assert_eq!(map.get("kind").map(String::as_str), Some("install"));
        assert_eq!(map.get("path").map(String::as_str), Some("/usr/bin/demo"));
        assert!(!map.contains_key("bad-line"));
    }
}
