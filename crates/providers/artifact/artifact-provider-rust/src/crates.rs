//! The crate graph a cargo build linked, recorded for the image checks when
//! the build declares a surface: each crate of the artifact package's normal
//! (runtime) dependencies, with the features cargo resolved for it. The
//! image checks hold it against the surfaces a program loads ("not both": a
//! surface's state linked statically and loaded as a library).
//!
//! `cargo tree -p <package> -e normal`, in the artifact's backend after its
//! build, `--offline`: the build fetched everything. Features resolve per
//! package, as the build of that package resolved them.

use std::process::Command;
use std::time::Duration;

use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    command_for_execution, command_output_with_timeout,
};

use crate::feature_flags::CargoFeatureFlags;

/// The record's format: bumped when its shape changes.
const RECORD_SCHEMA: u32 = 2;

/// After a cargo build, when the build declares a surface: the record.
pub(crate) fn record_after_build(
    artifact: &gaia_spec::ArtifactSpec,
    contract: &ArtifactExecutionContract,
    package: &str,
    build_mode: &str,
) -> Result<(), ArtifactProviderError> {
    if !contract.rust_record_crates || build_mode != "cargo" {
        return Ok(());
    }
    let source_dir = contract.source_dir.as_deref().unwrap_or(".");
    record(
        source_dir,
        package,
        &CargoFeatureFlags::of(artifact),
        contract,
    )
}

/// Writes the record of `package`'s crates next to the artifact's output.
fn record(
    source_dir: &str,
    package: &str,
    flags: &CargoFeatureFlags,
    contract: &ArtifactExecutionContract,
) -> Result<(), ArtifactProviderError> {
    let command = command_for_execution(
        &tree_command(source_dir, package, flags, contract),
        contract,
    )?;
    let mut command = command;
    let output = command_output_with_timeout(
        &mut command,
        Duration::from_secs(contract.timeout_seconds.max(1)),
        &format!("cargo tree for package '{package}'"),
    )?;
    if !output.status.success() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "cargo tree for package '{package}' failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let crates = parse_tree(&String::from_utf8_lossy(&output.stdout));
    let record = serde_json::json!({
        "schema": RECORD_SCHEMA,
        "package": package,
        "crates": crates
            .iter()
            .map(|entry| serde_json::json!({
                "name": entry.name,
                "version": entry.version,
                "source": entry.source,
                "features": entry.features,
            }))
            .collect::<Vec<_>>(),
    });
    let path = gaia_spec::rust_crates_record_path(std::path::Path::new(&contract.output.path));
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, format!("{record:#}\n"))
    };
    write().map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!("crate record '{}': {error}", path.display()),
        )
    })
}

fn tree_command(
    source_dir: &str,
    package: &str,
    flags: &CargoFeatureFlags,
    contract: &ArtifactExecutionContract,
) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["tree", "--offline", "-e", "normal", "--prefix", "none"])
        .args(["-f", "{p}|{f}"])
        .arg("-p")
        .arg(package);
    flags.apply(&mut command);
    if let Some(target) = contract.artifact_target.as_deref() {
        command.arg("--target").arg(target);
    }
    command.current_dir(source_dir);
    command
}

/// One crate of the graph: a name at a version from a source (a path or a
/// git URL with its commit; empty for a registry), and its features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrateUse {
    pub name: String,
    pub version: String,
    pub source: String,
    pub features: Vec<String>,
}

/// `name vX.Y.Z (source)|feature,feature` lines, a crate repeated where it
/// is reached again (`(*)`): each name, version and source once.
pub(crate) fn parse_tree(text: &str) -> Vec<CrateUse> {
    let mut crates = std::collections::BTreeMap::<(String, String, String), Vec<String>>::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((package, features)) = line.split_once('|') else {
            continue;
        };
        let mut words = package.splitn(3, ' ');
        let (Some(name), Some(version)) = (words.next(), words.next()) else {
            continue;
        };
        let source = words
            .next()
            .map(|rest| rest.trim().trim_start_matches('(').trim_end_matches(')'))
            .unwrap_or_default();
        let key = (
            name.to_string(),
            version.trim_start_matches('v').to_string(),
            source.to_string(),
        );
        let entry = crates.entry(key).or_default();
        let features = features
            .trim()
            .trim_end_matches("(*)")
            .split(',')
            .map(str::trim)
            .filter(|feature| !feature.is_empty());
        for feature in features {
            if !entry.iter().any(|known| known == feature) {
                entry.push(feature.to_string());
            }
        }
        entry.sort();
    }
    crates
        .into_iter()
        .map(|((name, version, source), features)| CrateUse {
            name,
            version,
            source,
            features,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_crate_is_read_once_per_version_and_source_with_its_features() {
        let text = "app v0.1.0 (/src/app)|\n\
            lemnos-capi v2.0.1 (/src/lemnos/crates/lemnos-capi)|default,shared\n\
            lemnos-abi v2.0.1 (/src/lemnos/crates/lemnos-abi)|std\n\
            serde v1.0.228|default,std\n\
            lemnos-abi v2.0.1 (/src/lemnos/crates/lemnos-abi)|std (*)\n\
            lemnos-abi v2.0.0 (https://github.com/o/lemnos.git?branch=dev#086d6ce)|std\n";
        let crates = parse_tree(text);
        let row = |entry: &CrateUse| {
            format!(
                "{} {} [{}] {}",
                entry.name,
                entry.version,
                entry.source,
                entry.features.join(",")
            )
        };
        assert_eq!(
            crates.iter().map(row).collect::<Vec<_>>(),
            [
                "app 0.1.0 [/src/app] ",
                "lemnos-abi 2.0.0 [https://github.com/o/lemnos.git?branch=dev#086d6ce] std",
                "lemnos-abi 2.0.1 [/src/lemnos/crates/lemnos-abi] std",
                "lemnos-capi 2.0.1 [/src/lemnos/crates/lemnos-capi] default,shared",
                "serde 1.0.228 [] default,std",
            ]
        );
    }
}
