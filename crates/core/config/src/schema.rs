//! The JSON Schema of a build file, for editors: `gaia config schema` prints
//! it, and taplo or VS Code's Even Better TOML check a file against it.
//!
//! The schema is written from the same key lists the unknown-key check uses
//! (see `unknown_keys`), so the two agree. Keys are named and checked (a
//! table takes only the keys its struct or its `kind` takes); the values are
//! not typed, since the raw structs do not carry a schema of their own.
//! Sections accept either a table or an array of tables, as the config does.

use serde_json::{Map, Value, json};

use crate::unknown_keys::layout::{self, keys_for_kind, nested, root_fields, top_sections};
use crate::unknown_keys::{FieldsFn, Known, Section};

const SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The JSON Schema of a build file, pretty-printed.
pub fn config_schema_json() -> String {
    serde_json::to_string_pretty(&config_schema()).expect("a JSON value always serializes")
}

fn config_schema() -> Value {
    let sections = top_sections();
    let mut properties = Map::new();
    for field in root_fields() {
        let schema = match sections.iter().find(|node| node.key == field) {
            Some(node) => section_schema(node.key, node.known, node.children, node.entries, None),
            None => json!({}),
        };
        properties.insert(field.to_string(), schema);
    }
    json!({
        "$schema": SCHEMA_DRAFT,
        "title": "Gaia build config",
        "description": "A Gaia build file (build.toml). Keys Gaia does not know are reported when the build is validated.",
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    })
}

/// A section: a table, or an array of tables. A kinded section is one table
/// shape per kind it names; `kind` restricts a single shape to that kind.
/// `entries` is set for a named-table section (`[secrets]`): its own keys are
/// the names of tables that each take `entries`.
fn section_schema(
    key: &str,
    known: Known,
    children: &[Section],
    entries: Option<FieldsFn>,
    kind: Option<&str>,
) -> Value {
    let table = match known {
        Known::ByKind(_) if kind.is_none() => {
            let shapes = layout::kinds_of(key)
                .iter()
                .map(|kind| table_schema(known, children, None, Some(kind)))
                .collect::<Vec<_>>();
            json!({ "anyOf": shapes })
        }
        _ => table_schema(known, children, entries, kind),
    };
    json!({ "anyOf": [table.clone(), { "type": "array", "items": table }] })
}

/// One table shape: the keys it takes, each unconstrained, and its nested
/// sections. A table whose keys are not known accepts any key.
fn table_schema(
    known: Known,
    children: &[Section],
    entries: Option<FieldsFn>,
    kind: Option<&str>,
) -> Value {
    let mut properties = Map::new();
    let fields = keys_for_kind(known, kind);
    if let Some(fields) = &fields {
        for field in fields {
            properties.insert(field.to_string(), json!({}));
        }
    }
    if let Some(kind) = kind {
        properties.insert("kind".to_string(), json!({ "const": kind }));
    }
    for child in children {
        let (key, child_known, grandchildren, child_entries) = nested(child);
        properties.insert(
            key.to_string(),
            section_schema(key, child_known, grandchildren, child_entries, None),
        );
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if let Some(entry) = entries {
        // Each key names a table, and that table takes the entry's keys.
        schema.insert(
            "additionalProperties".into(),
            table_schema(Known::Fixed(entry), &[], None, None),
        );
    } else if fields.is_some() {
        schema.insert("additionalProperties".into(), json!(false));
    }
    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The problems `instance` has against `schema`, in the subset of JSON
    /// Schema this schema uses: `anyOf`, `const`, `type`, `properties`,
    /// `additionalProperties: false` and `items`.
    fn problems(instance: &Value, schema: &Value, at: &str) -> Vec<String> {
        if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
            let mut failures = Vec::new();
            for branch in branches {
                let found = problems(instance, branch, at);
                if found.is_empty() {
                    return Vec::new();
                }
                failures.push(found.join("; "));
            }
            return vec![format!("{at}: no shape fits ({})", failures.join(" | "))];
        }
        if let Some(expected) = schema.get("const")
            && instance != expected
        {
            return vec![format!("{at}: expected {expected}")];
        }
        let mut found = Vec::new();
        match schema.get("type").and_then(Value::as_str) {
            Some("object") => {
                let Some(table) = instance.as_object() else {
                    return vec![format!("{at}: expected a table")];
                };
                let properties = schema.get("properties").and_then(Value::as_object);
                match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        for key in table.keys() {
                            if properties.is_none_or(|known| !known.contains_key(key)) {
                                found.push(format!("{at}.{key}: unknown key"));
                            }
                        }
                    }
                    Some(extra @ Value::Object(_)) => {
                        for (key, value) in table {
                            if properties.is_none_or(|known| !known.contains_key(key)) {
                                found.extend(problems(value, extra, &format!("{at}.{key}")));
                            }
                        }
                    }
                    _ => {}
                }
                for (key, child_schema) in properties.into_iter().flatten() {
                    if let Some(value) = table.get(key) {
                        found.extend(problems(value, child_schema, &format!("{at}.{key}")));
                    }
                }
            }
            Some("array") => {
                let Some(items) = instance.as_array() else {
                    return vec![format!("{at}: expected an array")];
                };
                let item_schema = schema.get("items").cloned().unwrap_or(json!({}));
                for (index, item) in items.iter().enumerate() {
                    found.extend(problems(item, &item_schema, &format!("{at}[{index}]")));
                }
            }
            _ => {}
        }
        found
    }

    fn schema() -> Value {
        config_schema()
    }

    /// Every `.toml` file under `dir`, recursively. Build output left in an
    /// example by a local run (gitignored `build`, `out`, `.gaia`, `target`,
    /// `seed`) is skipped, and so is anything unreadable in it.
    fn toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let generated = matches!(
                entry.file_name().to_str(),
                Some("build" | "out" | ".gaia" | "target" | "seed")
            );
            if generated {
                continue;
            }
            if path.is_dir() {
                toml_files(&path, out);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                out.push(path);
            }
        }
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    #[test]
    fn the_schema_names_every_section_the_build_file_has() {
        let schema = schema();
        let properties = schema["properties"].as_object().expect("properties");
        for key in ["sources", "artifacts", "image", "workspace", "providers"] {
            assert!(properties.contains_key(key), "{key} is in the schema");
        }
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn a_secret_entry_takes_only_its_source_keys() {
        let schema = schema();
        let good: toml::Value =
            toml::from_str("[ghcr]\ncommand = [\"gh\", \"auth\", \"token\"]\nrequired = false\n")
                .expect("toml");
        let bad: toml::Value = toml::from_str("[ghcr]\ncomand = [\"gh\"]\n").expect("toml");
        let good = serde_json::to_value(good).expect("json");
        let bad = serde_json::to_value(bad).expect("json");
        let secrets = &schema["properties"]["secrets"];
        assert!(problems(&good, secrets, "secrets").is_empty());
        assert!(
            problems(&bad, secrets, "secrets")
                .iter()
                .any(|problem| problem.contains("secrets.ghcr.comand")),
            "{:?}",
            problems(&bad, secrets, "secrets")
        );
    }

    #[test]
    fn a_source_takes_only_its_kinds_keys() {
        let schema = schema();
        let good: toml::Value = toml::from_str(
            "[[sources]]\nid = \"s\"\nkind = \"git\"\nrepo = \"r\"\nbranch = \"main\"\n",
        )
        .expect("toml");
        let bad: toml::Value =
            toml::from_str("[[sources]]\nid = \"s\"\nkind = \"path\"\nrepo = \"r\"\n")
                .expect("toml");
        let good = serde_json::to_value(good).expect("json");
        let bad = serde_json::to_value(bad).expect("json");
        assert!(problems(&good, &schema, "").is_empty());
        assert!(
            problems(&bad, &schema, "")
                .iter()
                .any(|problem| problem.contains("sources[0].repo")),
            "{:?}",
            problems(&bad, &schema, "")
        );
    }

    /// Whether a TOML file is a build config (or a layer of one), rather than
    /// the manifest of a project the build compiles.
    fn is_build_config(path: &Path) -> bool {
        !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Cargo.toml" | "pyproject.toml" | "config.toml")
        )
    }

    #[test]
    fn every_example_config_validates() {
        let mut files = Vec::new();
        toml_files(&repo_root().join("examples"), &mut files);
        files.retain(|file| is_build_config(file));
        assert!(files.len() >= 10, "the examples are found: {files:?}");
        let schema = schema();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("example");
            let value: toml::Value = toml::from_str(&text).expect("example parses");
            let instance = serde_json::to_value(value).expect("json");
            let found = problems(&instance, &schema, "");
            assert!(found.is_empty(), "{}: {found:?}", file.display());
        }
    }
}
