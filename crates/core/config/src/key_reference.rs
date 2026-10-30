//! The build file key reference (`docs/reference/config-keys.md`): every
//! table and the keys it takes, written from the same layout as the JSON
//! schema and the unknown-key check, so it lists exactly what Gaia accepts.
//! `gaia config keys` prints it; a test keeps the committed copy current.

use std::fmt::Write as _;

use crate::unknown_keys::layout::{self, keys_for_kind, nested, root_fields, top_sections};
use crate::unknown_keys::{FieldsFn, Known, Section};

/// The docs page that describes a table, by its path (the longest prefix
/// listed wins). Paths are relative to `docs/reference/`.
const PAGES: &[(&str, &str)] = &[
    ("", "../configuration.md#top-level-build-fields"),
    ("product", "../configuration.md#product-metadata"),
    ("expect", "../configuration.md#top-level-build-fields"),
    ("interpolation", "../configuration.md#interpolation"),
    ("secrets", "../configuration.md#secrets"),
    ("imports", "../configuration.md#file-loading"),
    ("inputs", "../configuration-inputs.md#inputs"),
    ("presets", "../configuration-inputs.md#presets"),
    ("variants", "../configuration-inputs.md#variants"),
    ("failure", "../configuration-policy.md#failure-policy"),
    ("execution", "../configuration-policy.md#execution-policy"),
    (
        "providers",
        "../configuration-policy.md#provider-execution-policy",
    ),
    ("provenance", "../configuration-policy.md#provenance"),
    ("workspace", "../configuration-policy.md#workspace"),
    ("clean", "../configuration-policy.md#clean"),
    ("sources", "../configuration-sources.md#sources"),
    ("toolchains", "../configuration-sources.md#toolchains"),
    ("artifacts", "../configuration-artifacts.md#artifacts"),
    ("install", "../configuration-artifacts.md#install"),
    ("stage", "../configuration-artifacts.md#stage"),
    ("image", "../configuration-image.md#image"),
    ("image.feed", "../configuration-image.md#image-feed"),
    ("image.signing", "../configuration-image.md#signing"),
    ("image.layers", "../configuration-image.md#image-layers"),
    ("image.checks", "../configuration-image.md#image-checks"),
    ("image.hardware", "../configuration-image.md#image-checks"),
    (
        "image.assembly",
        "../configuration-assembly.md#image-assembly",
    ),
    (
        "image.assembly.checks",
        "../configuration-assembly.md#assembly-checks",
    ),
    ("checkpoints", "../configuration-checkpoints.md#checkpoints"),
    ("reporting", "../configuration-checkpoints.md#reporting"),
];

/// The tables written as arrays of tables (`[[sources]]`); every other
/// table is a single `[table]`.
const ARRAYS: &[&str] = &[
    "sources",
    "toolchains",
    "artifacts",
    "install",
    "checkpoints",
    "variants",
    "workspace.named_paths",
    "stage.files",
    "stage.env_sets",
    "stage.services",
    "image.expected_images",
    "image.hardware",
    "image.assembly.trees",
    "image.assembly.dirs",
    "image.assembly.symlinks",
    "image.assembly.files",
    "image.assembly.transforms",
    "image.assembly.filesystems",
    "image.assembly.disks",
    "image.assembly.disks.partitions",
    "image.assembly.archives",
    "image.assembly.archives.members",
    "image.assembly.archives.generated",
    "image.assembly.oci",
    "image.assembly.packages",
    "image.assembly.checks",
    "image.assembly.busybox_initramfs",
    "image.assembly.binaries",
    "image.assembly.kernel_modules",
    "image.assembly.devices",
];

/// The key reference as Markdown.
pub fn config_key_reference_markdown() -> String {
    let sections = top_sections();
    let mut out = String::new();
    out.push_str(
        "# Build file key reference\n\n\
         Every table a build file can have and the keys it takes. This page is\n\
         generated from the config layout Gaia validates against (`gaia config keys`\n\
         prints it); do not edit it by hand. Each table links to the page that\n\
         describes it. A key not listed here is reported as unknown when the build\n\
         is validated.\n",
    );
    let (tables, plain): (Vec<_>, Vec<_>) = root_fields()
        .into_iter()
        .partition(|field| sections.iter().any(|node| node.key == *field));
    heading(&mut out, "top level", "", None);
    keys_line(&mut out, Some(plain));
    for field in tables {
        let node = sections
            .iter()
            .find(|node| node.key == field)
            .expect("partitioned on it");
        section(&mut out, node.key, node.known, node.children, node.entries);
    }
    out
}

fn section(
    out: &mut String,
    path: &str,
    known: Known,
    children: &[Section],
    entries: Option<FieldsFn>,
) {
    let key = path.rsplit('.').next().unwrap_or(path);
    let kinds: Vec<Option<&str>> = match known {
        Known::ByKind(_) => layout::kinds_of(key).iter().map(|k| Some(*k)).collect(),
        Known::Fixed(_) => vec![None],
    };
    for kind in kinds {
        let title = if ARRAYS.contains(&path) {
            format!("[[{path}]]")
        } else {
            format!("[{path}]")
        };
        heading(out, &title, path, kind);
        let child_keys = children.iter().map(|child| nested(child).0);
        let keys = keys_for_kind(known, kind).map(|keys| {
            keys.into_iter()
                .filter(|key| {
                    (kind.is_none() || *key != "kind")
                        && !child_keys.clone().any(|child| child == *key)
                })
                .collect::<Vec<_>>()
        });
        keys_line(out, keys);
        if let Some(entry) = entries {
            let _ = write!(out, "\nEach `[{path}.<name>]` takes: ");
            keys_list(out, entry().map(<[&str]>::to_vec));
        }
    }
    for child in children {
        let (child_key, child_known, grandchildren, child_entries) = nested(child);
        section(
            out,
            &format!("{path}.{child_key}"),
            child_known,
            grandchildren,
            child_entries,
        );
    }
}

fn heading(out: &mut String, title: &str, path: &str, kind: Option<&str>) {
    let kind = kind
        .map(|kind| format!(" with `kind = \"{kind}\"`"))
        .unwrap_or_default();
    let _ = write!(
        out,
        "\n## `{title}`{kind}\n\nDescribed in [{}]({}).\n",
        page_title(path),
        page(path)
    );
}

fn keys_line(out: &mut String, keys: Option<Vec<&str>>) {
    out.push_str("\nKeys: ");
    keys_list(out, keys);
}

fn keys_list(out: &mut String, keys: Option<Vec<&str>>) {
    match keys {
        Some(mut keys) if !keys.is_empty() => {
            keys.sort_unstable();
            let names = keys
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "{names}.");
        }
        Some(_) => out.push_str("none of its own.\n"),
        None => out.push_str("any key.\n"),
    }
}

fn page(path: &str) -> &'static str {
    PAGES
        .iter()
        .filter(|(prefix, _)| {
            prefix.is_empty() || path == *prefix || path.starts_with(&format!("{prefix}."))
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, page)| *page)
        .expect("the empty prefix matches every path")
}

fn page_title(path: &str) -> &'static str {
    let page = page(path);
    let file = page.split('#').next().unwrap_or(page);
    match file {
        "../configuration.md" => "Configuration",
        "../configuration-inputs.md" => "Inputs, presets, variants and profiles",
        "../configuration-policy.md" => "Policy, workspace and clean",
        "../configuration-sources.md" => "Sources and toolchains",
        "../configuration-artifacts.md" => "Artifacts, install and stage",
        "../configuration-image.md" => "Image",
        "../configuration-assembly.md" => "Image assembly",
        _ => "Checkpoints, registry and reporting",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn reference_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference/config-keys.md")
    }

    /// The committed reference is what the layout generates. Run the test
    /// with `GAIA_UPDATE_DOCS=1` to rewrite it after a config change.
    #[test]
    fn the_committed_key_reference_is_current() {
        let generated = config_key_reference_markdown();
        let path = reference_path();
        if std::env::var_os("GAIA_UPDATE_DOCS").is_some() {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("reference dir");
            std::fs::write(&path, &generated).expect("write reference");
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == generated,
            "docs/reference/config-keys.md is out of date: run \
             `GAIA_UPDATE_DOCS=1 cargo test -p gaia-config key_reference`"
        );
    }

    /// Every key Gaia accepts is mentioned somewhere in the docs (outside
    /// the generated reference), so a new key needs its documentation.
    #[test]
    fn every_key_is_documented() {
        let docs_dir = reference_path()
            .parent()
            .and_then(Path::parent)
            .expect("docs dir")
            .to_path_buf();
        let mut docs = String::new();
        for entry in std::fs::read_dir(&docs_dir).expect("docs dir").flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "md") {
                docs.push_str(&std::fs::read_to_string(&path).expect("doc"));
            }
        }
        let mentioned = |key: &str| {
            docs.match_indices(key).any(|(at, _)| {
                let before = docs[..at].chars().next_back();
                let after = docs[at + key.len()..].chars().next();
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                !word(before) && !word(after)
            })
        };
        let reference = config_key_reference_markdown();
        let mut missing = Vec::new();
        for section in reference.split("\n## ").skip(1) {
            let title = section.lines().next().unwrap_or_default();
            for line in section.lines() {
                let Some((label, keys)) = line.split_once(": ") else {
                    continue;
                };
                if label != "Keys" && !label.starts_with("Each") {
                    continue;
                }
                for key in keys.split(", ") {
                    let key = key.trim_end_matches('.').trim_matches('`');
                    if key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        && !mentioned(key)
                    {
                        missing.push(format!("{title}: {key}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "keys no doc mentions: {missing:#?}");
    }

    #[test]
    fn every_table_links_to_a_page_section() {
        let text = config_key_reference_markdown();
        assert!(
            text.contains("## `[[sources]]` with `kind = \"git\"`"),
            "{text}"
        );
        assert!(text.contains("](../configuration-assembly.md#image-assembly)"));
        assert!(!text.contains("Keys: any key.\n\n## `top level`"));
    }
}
