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
const RECORD_SCHEMA: u32 = 1;

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
            .map(|(name, features)| serde_json::json!({"name": name, "features": features}))
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

/// `name vX.Y.Z (path)|feature,feature` lines, a crate repeated where it
/// is reached again (`(*)`): each crate once, with its features.
pub(crate) fn parse_tree(text: &str) -> Vec<(String, Vec<String>)> {
    let mut crates = std::collections::BTreeMap::<String, Vec<String>>::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((package, features)) = line.split_once('|') else {
            continue;
        };
        let Some(name) = package.split_whitespace().next() else {
            continue;
        };
        let features = features
            .trim()
            .trim_end_matches("(*)")
            .split(',')
            .map(str::trim)
            .filter(|feature| !feature.is_empty())
            .map(str::to_string);
        let entry = crates.entry(name.to_string()).or_default();
        for feature in features {
            if !entry.contains(&feature) {
                entry.push(feature);
            }
        }
        entry.sort();
    }
    crates.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_crate_is_read_once_with_its_features() {
        let text = "app v0.1.0 (/src/app)|\n\
            lemnos-capi v2.0.1 (/src/lemnos/crates/lemnos-capi)|default,shared\n\
            lemnos-abi v2.0.1 (/src/lemnos/crates/lemnos-abi)|std\n\
            serde v1.0.228|default,std\n\
            lemnos-abi v2.0.1 (/src/lemnos/crates/lemnos-abi)|std (*)\n";
        assert_eq!(
            parse_tree(text),
            vec![
                ("app".to_string(), vec![]),
                ("lemnos-abi".to_string(), vec!["std".to_string()]),
                (
                    "lemnos-capi".to_string(),
                    vec!["default".to_string(), "shared".to_string()]
                ),
                (
                    "serde".to_string(),
                    vec!["default".to_string(), "std".to_string()]
                ),
            ]
        );
    }
}
