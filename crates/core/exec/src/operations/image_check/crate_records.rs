//! The crate records the Rust provider writes next to each artifact when the
//! build declares a surface (`gaia_spec::rust_crates_record_path`), and the
//! check that the image holds each of its own crates once.
//!
//! "Its own": a crate from a path or a git source, the product's and its
//! dependencies' repositories. Two versions or sources of one of them in an
//! image (two Lemnos checkouts in one lock, a program built from another
//! workspace) is `crate_version_split`, an error unless
//! `[[image.checks.crate_splits]]` allows that crate with a reason. Registry
//! crates are left out: two semver-incompatible versions of a generic crate
//! are cargo's normal business.

use super::kernel_modules::Outcome;
use super::surfaces::installed_rust;
use gaia_spec::{ArtifactSpec, ResolvedBuildSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One crate of an artifact's graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedCrate {
    pub name: String,
    pub version: String,
    /// A path or a git URL with its commit; empty for a registry crate.
    pub source: String,
    pub features: Vec<String>,
}

pub(super) fn record_path(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> PathBuf {
    let output = gaia_spec::resolve_workspace_path(&spec.workspace, &artifact.output.path)
        .unwrap_or_else(|_| PathBuf::from(&artifact.output.path));
    gaia_spec::rust_crates_record_path(&output)
}

/// The record's crates; `None` when it is missing or not one Gaia reads.
pub(super) fn read_record(path: &Path) -> Option<Vec<RecordedCrate>> {
    let text = std::fs::read_to_string(path).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    let text_of = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(
        record
            .get("crates")?
            .as_array()?
            .iter()
            .filter_map(|entry| {
                Some(RecordedCrate {
                    name: entry.get("name")?.as_str()?.to_string(),
                    version: text_of(entry, "version"),
                    source: text_of(entry, "source"),
                    features: entry
                        .get("features")?
                        .as_array()?
                        .iter()
                        .filter_map(|feature| feature.as_str().map(str::to_string))
                        .collect(),
                })
            })
            .collect(),
    )
}

/// Crate names compare with `-` and `_` as one, as cargo's do.
pub(super) fn same_crate(a: &str, b: &str) -> bool {
    a.replace('_', "-") == b.replace('_', "-")
}

/// Fails each own crate the installed artifacts resolved to more than one
/// version or source, unless the image allows it.
pub(super) fn check_splits(spec: &ResolvedBuildSpec, rootfs: &Path, outcome: &mut Outcome) {
    // name -> (version, source) -> artifacts
    let mut seen: BTreeMap<String, BTreeMap<(String, String), BTreeSet<String>>> = BTreeMap::new();
    let mut artifacts = BTreeSet::new();
    for (artifact, _) in installed_rust(spec, rootfs) {
        if !artifacts.insert(artifact.id.as_str().to_string()) {
            continue;
        }
        let Some(crates) = read_record(&record_path(spec, artifact)) else {
            continue;
        };
        for entry in crates {
            seen.entry(entry.name.replace('_', "-"))
                .or_default()
                .entry((entry.version, entry.source))
                .or_default()
                .insert(artifact.id.as_str().to_string());
        }
    }
    for (name, forms) in seen {
        if forms.len() < 2 || forms.keys().all(|(_, source)| source.is_empty()) {
            continue;
        }
        if spec
            .image
            .checks
            .crate_splits
            .iter()
            .any(|split| same_crate(&split.crate_name, &name))
        {
            continue;
        }
        let described = forms
            .iter()
            .map(|((version, source), users)| {
                let from = if source.is_empty() {
                    "the registry"
                } else {
                    source
                };
                format!(
                    "{version} from {from} ({})",
                    users
                        .iter()
                        .map(|id| format!("'{id}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        outcome.error(
            "crate_version_split",
            Some(&name),
            format!(
                "crate '{name}' is in the image at {} versions or sources: {described}; build them from one lock, or allow it with [[image.checks.crate_splits]] crate = \"{name}\" and a reason",
                forms.len()
            ),
        );
    }
}
