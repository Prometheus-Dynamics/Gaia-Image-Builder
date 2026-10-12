//! `when` conditions: which imports, artifacts, install and stage items a
//! build selects, by target, profile, branch, image kind or input value.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, de};

use crate::raw::RawBuildConfig;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RawWhenConfig {
    pub target: Option<String>,
    pub profile: Option<String>,
    pub branch: Option<String>,
    pub image_kind: Option<RawWhenImageKind>,
    /// `input = { name = "value" }`: every listed input must have this
    /// value (its string form). The names must be declared inputs.
    #[serde(deserialize_with = "deserialize_input_values")]
    pub input: BTreeMap<String, String>,
    pub all: Vec<RawWhenConfig>,
    pub any: Vec<RawWhenConfig>,
    pub not: Option<Box<RawWhenConfig>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawWhenImageKind {
    Buildroot,
    StartingPoint,
}

/// Input values in a `when.input` table, as their string form: a boolean or
/// number is written as it reads in TOML.
fn deserialize_input_values<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = BTreeMap::<String, toml::Value>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|(name, value)| match value {
            toml::Value::String(text) => Ok((name, text)),
            toml::Value::Boolean(flag) => Ok((name, flag.to_string())),
            toml::Value::Integer(number) => Ok((name, number.to_string())),
            other => Err(de::Error::custom(format!(
                "when.input '{name}' must be a string, boolean or integer, not {}",
                other.type_str()
            ))),
        })
        .collect()
}

/// Every `when.input` name in the build tree (the file, its `extends` chain
/// and its imports, whether or not they apply) must name an input some file
/// in the tree declares. Returns the first name that no file declares.
pub(crate) fn unknown_when_input(raw: &RawBuildConfig) -> Option<String> {
    let mut declared = std::collections::BTreeSet::new();
    let mut used = Vec::new();
    collect_tree(raw, &mut declared, &mut used);
    used.into_iter().find(|name| !declared.contains(name))
}

fn collect_tree(
    raw: &RawBuildConfig,
    declared: &mut std::collections::BTreeSet<String>,
    used: &mut Vec<String>,
) {
    declared.extend(raw.inputs.keys().cloned());
    let mut whens = Vec::new();
    for import in &raw.imports {
        whens.extend(import.when.as_ref());
    }
    for artifact in &raw.artifacts {
        whens.extend(artifact.when.as_ref());
    }
    for install in &raw.install {
        whens.extend(install.when.as_ref());
    }
    for file in &raw.stage.files {
        whens.extend(file.when.as_ref());
    }
    for env_set in &raw.stage.env_sets {
        whens.extend(env_set.when.as_ref());
    }
    for service in &raw.stage.services {
        whens.extend(service.when.as_ref());
    }
    for when in whens {
        push_input_names(when, used);
    }
    if let Some(extends) = raw.extends_config.as_deref() {
        collect_tree(extends, declared, used);
    }
    for imported in &raw.imported_configs {
        collect_tree(&imported.config, declared, used);
    }
}

fn push_input_names(when: &RawWhenConfig, used: &mut Vec<String>) {
    used.extend(when.input.keys().cloned());
    for item in when.all.iter().chain(when.any.iter()) {
        push_input_names(item, used);
    }
    if let Some(item) = when.not.as_deref() {
        push_input_names(item, used);
    }
}
