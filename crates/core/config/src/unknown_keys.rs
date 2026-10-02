//! Keys a build file sets that this Gaia does not know.
//!
//! The raw config structs accept and silently drop unknown keys (so files
//! written for a newer Gaia still load). Validation reports them as
//! warnings instead. Known keys are taken from the serde field lists of the
//! raw structs themselves, so the check never drifts from the parser.
//! Sections whose struct flattens another (sources, artifacts, image) or
//! holds free-form maps (inputs, presets, env) are not checked.
use std::path::Path;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

use crate::raw::{self, RawBuildConfig};

type FieldsFn = fn() -> Option<&'static [&'static str]>;

/// A table (or array of tables) and the struct it deserializes into.
struct Section {
    key: &'static str,
    fields: FieldsFn,
    children: &'static [Section],
}

const fn section(key: &'static str, fields: FieldsFn, children: &'static [Section]) -> Section {
    Section {
        key,
        fields,
        children,
    }
}

const COMMAND_PROVIDER_CHILDREN: &[Section] = &[section(
    "ccache",
    struct_fields::<raw::RawBuildrootCcachePolicyConfig>,
    &[],
)];

const SECTIONS: &[Section] = &[
    section("workspace", struct_fields::<raw::RawWorkspaceConfig>, &[]),
    section("product", struct_fields::<raw::RawProductConfig>, &[]),
    section(
        "interpolation",
        struct_fields::<raw::RawInterpolationConfig>,
        &[],
    ),
    section("clean", struct_fields::<raw::RawCleanConfig>, &[]),
    section(
        "execution",
        struct_fields::<raw::RawExecutionPolicyConfig>,
        &[
            section(
                "docker",
                struct_fields::<raw::RawDockerExecutionConfig>,
                &[],
            ),
            section(
                "output_retention",
                struct_fields::<raw::RawOutputRetentionPolicyConfig>,
                &[],
            ),
        ],
    ),
    section("failure", struct_fields::<raw::RawFailurePolicyConfig>, &[]),
    section(
        "providers",
        struct_fields::<raw::RawProviderPoliciesConfig>,
        &[
            section(
                "rust",
                struct_fields::<raw::RawRustProviderPolicyConfig>,
                &[],
            ),
            section("git", struct_fields::<raw::RawGitProviderPolicyConfig>, &[]),
            section(
                "archive",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "download",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "go",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "java",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "node",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "python",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "buildroot",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "starting_point",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
        ],
    ),
    section(
        "provenance",
        struct_fields::<raw::RawProvenanceConfig>,
        &[section(
            "identity",
            struct_fields::<raw::RawProvenanceIdentityConfig>,
            &[],
        )],
    ),
    section(
        "reporting",
        struct_fields::<raw::RawReportingConfig>,
        &[
            section(
                "masking",
                struct_fields::<raw::RawReportingMaskingConfig>,
                &[],
            ),
            section(
                "output_hygiene",
                struct_fields::<raw::RawOutputHygieneConfig>,
                &[],
            ),
            section(
                "post_build",
                struct_fields::<raw::RawPostBuildHookConfig>,
                &[],
            ),
        ],
    ),
    section(
        "stage",
        struct_fields::<raw::RawStageConfig>,
        &[
            section("files", struct_fields::<raw::RawStageFileConfig>, &[]),
            section("env_sets", struct_fields::<raw::RawStageEnvSetConfig>, &[]),
            section("services", struct_fields::<raw::RawStageServiceConfig>, &[]),
        ],
    ),
    section("install", struct_fields::<raw::RawInstallConfig>, &[]),
    section(
        "checkpoints",
        struct_fields::<raw::RawCheckpointConfig>,
        &[],
    ),
];

/// Dotted paths of the unknown keys in one parsed build file.
pub(crate) fn unknown_config_keys(value: &toml::Value) -> Vec<String> {
    let mut unknown = Vec::new();
    check_table(
        value,
        "",
        struct_fields::<RawBuildConfig>(),
        SECTIONS,
        &mut unknown,
    );
    unknown.sort();
    unknown
}

/// Warning messages for every unknown key in the loaded file, its
/// `extends` chain and its imports.
pub(crate) fn collect_unknown_key_warnings(raw: &RawBuildConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    collect_into(raw, &mut warnings);
    warnings
}

fn collect_into(raw: &RawBuildConfig, warnings: &mut Vec<String>) {
    let path = raw
        .source_path
        .as_deref()
        .map(Path::display)
        .map(|path| path.to_string())
        .unwrap_or_else(|| "<build config>".into());
    for key in &raw.unknown_keys {
        let warning = format!(
            "unknown key '{key}' in '{path}' is ignored; it may need a newer gaia \
             (pin one with gaia_version = \">=X.Y.Z\")"
        );
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    if let Some(extends) = &raw.extends_config {
        collect_into(extends, warnings);
    }
    for imported in &raw.imported_configs {
        collect_into(&imported.config, warnings);
    }
}

fn check_table(
    value: &toml::Value,
    prefix: &str,
    known: Option<&'static [&'static str]>,
    children: &'static [Section],
    unknown: &mut Vec<String>,
) {
    let Some(known) = known else {
        return;
    };
    let tables: Vec<(String, &toml::Table)> = match value {
        toml::Value::Table(table) => vec![(prefix.to_string(), table)],
        toml::Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                item.as_table()
                    .map(|table| (format!("{prefix}[{index}]"), table))
            })
            .collect(),
        _ => return,
    };
    for (path, table) in tables {
        for (key, child) in table {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if !known.contains(&key.as_str()) {
                unknown.push(child_path);
                continue;
            }
            if let Some(section) = children.iter().find(|section| section.key == key) {
                check_table(
                    child,
                    &child_path,
                    (section.fields)(),
                    section.children,
                    unknown,
                );
            }
        }
    }
}

/// The field names serde's derive passes to `deserialize_struct`, or `None`
/// for types that do not deserialize as a plain struct (flatten, custom
/// impls).
fn struct_fields<T: DeserializeOwned>() -> Option<&'static [&'static str]> {
    match T::deserialize(FieldNames) {
        Err(Captured(fields)) => fields,
        Ok(_) => None,
    }
}

#[derive(Debug)]
struct Captured(Option<&'static [&'static str]>);

impl std::fmt::Display for Captured {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("field name capture")
    }
}

impl std::error::Error for Captured {}

impl de::Error for Captured {
    fn custom<T: std::fmt::Display>(_message: T) -> Self {
        Self(None)
    }
}

struct FieldNames;

impl<'de> Deserializer<'de> for FieldNames {
    type Error = Captured;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Captured> {
        Err(Captured(None))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Captured> {
        Err(Captured(Some(fields)))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(text: &str) -> Vec<String> {
        unknown_config_keys(&toml::from_str(text).expect("toml"))
    }

    #[test]
    fn known_keys_are_accepted() {
        assert!(
            unknown(
                "gaia_version = \">=2.1.0\"\nbuild_name = \"demo\"\n\
                 [workspace]\nroot_dir = \".\"\n\
                 [providers.buildroot]\noverride_check = \"warn\"\n\
                 [providers.buildroot.ccache]\nenabled = true\n\
                 [[stage.files]]\nid = \"a\"\nsrc = \"a\"\ndest = \"/a\"\n\
                 [[sources]]\nid = \"s\"\nkind = \"path\"\npath = \".\"\nwhatever = 1\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn unknown_top_level_and_section_keys_are_reported() {
        assert_eq!(
            unknown(
                "build_command = \"make\"\n\
                 [providers.buildroot]\nshared_outptu = true\n\
                 [providers.buildroot.ccache]\nsize = 1\n\
                 [[stage.files]]\nid = \"a\"\nsrc = \"a\"\ndest = \"/a\"\nowner = \"root\"\n\
                 [future_section]\nkey = 1\n"
            ),
            vec![
                "build_command",
                "future_section",
                "providers.buildroot.ccache.size",
                "providers.buildroot.shared_outptu",
                "stage.files[0].owner",
            ]
        );
    }

    #[test]
    fn flattened_and_custom_types_are_not_checked() {
        assert!(struct_fields::<raw::RawSourceConfig>().is_none());
        assert!(struct_fields::<raw::RawImageConfig>().is_none());
        assert!(struct_fields::<RawBuildConfig>().is_some_and(|fields| {
            fields.contains(&"gaia_version") && !fields.contains(&"unknown_keys")
        }));
    }
}
