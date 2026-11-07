//! Relinking a trimmed library (`trim = { users = [...] }`) to export only
//! what its users import. The build group's cargo build has linked it in
//! full and `link_record` kept that link; here the users' imports are read
//! from their cargo files, rustc's version script is cut down to the names
//! among them the library defines (and the crate metadata symbol other
//! crates build against), and the link is run again with `--gc-sections`,
//! so the code only the dropped exports reached goes too.

use crate::link_record::{LinkRecord, read_record};
use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    ProcessCancelCheck, artifact_output_path, copy_artifact_file_to_output, dynamic_symbols,
    run_command_with_retries,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Relinks `artifact` when it is trimmed, from the cargo build in
/// `target_dir` whose outputs are in `profile_dir`, and writes the result to
/// its output. Returns what it did, or nothing for an artifact without users.
pub(crate) fn trim(
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    target_dir: &Path,
    profile_dir: &Path,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Option<String>, ArtifactProviderError> {
    let ArtifactDefinition::Rust(rust) = &artifact.definition else {
        return Ok(None);
    };
    let Some(spec) = rust.trim.as_ref().filter(|trim| !trim.users.is_empty()) else {
        return Ok(None);
    };
    let id = artifact.id.as_str();
    let failed = |message: String| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!("rust artifact '{id}' trim: {message}"),
        )
    };
    let source_dir = contract.source_dir.as_deref().unwrap_or(".");
    let output = artifact_output_path(contract, source_dir);
    let name = output_name(rust.target_name.as_deref(), &output);
    let linked = profile_dir.join(&name);
    let record = read_record(target_dir, &name).map_err(failed)?;
    if modified(&record.dir) < modified(&linked) {
        return Err(failed(format!(
            "'{}' is newer than its recorded link: cargo linked it without Gaia's wrapper; delete it to relink",
            linked.display()
        )));
    }
    let exports = read_symbols(&linked).map_err(failed)?.exports;
    let mut imports = BTreeSet::new();
    for file in &spec.user_files {
        let path = profile_dir.join(file);
        let symbols = read_symbols(&path).map_err(failed)?;
        imports.extend(symbols.imports.into_iter().map(|import| import.name));
    }
    let keep = exports
        .intersection(&imports)
        .cloned()
        .collect::<BTreeSet<_>>();

    let script = fs::read_to_string(record.dir.join("rustc/list"))
        .map_err(|error| failed(format!("the recorded version script: {error}")))?;
    let map = record.dir.join("trim.map");
    fs::write(&map, trimmed_script(&script, &keep))
        .map_err(|error| failed(format!("cannot write '{}': {error}", map.display())))?;
    let relinked = record.dir.join("trimmed").join(&name);
    fs::create_dir_all(relinked.parent().unwrap_or(&record.dir))
        .map_err(|error| failed(error.to_string()))?;
    let mut command = Command::new(&record.linker);
    command
        .args(replay_args(&record, &map, &relinked))
        .current_dir(source_dir);
    run_command_with_retries(
        &command,
        contract,
        &format!("relink of '{name}' exporting what its users import"),
        None,
        cancel_check,
    )?;

    let trimmed = read_symbols(&relinked).map_err(failed)?.exports;
    let missing = keep.difference(&trimmed).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(failed(format!(
            "the relinked library lacks {} name(s) its users import: {}",
            missing.len(),
            missing.join(", ")
        )));
    }
    copy_artifact_file_to_output(&relinked, &output, "trimmed rust library")?;
    if contract.rust_strip {
        crate::tools::strip_collected_output(&output, contract)?;
    }
    Ok(Some(format!(
        "rust artifact '{id}' trimmed for {} user(s): exports {} of {}, {} -> {} bytes",
        spec.users.len(),
        trimmed.len(),
        exports.len(),
        size(&linked),
        size(&relinked)
    )))
}

fn output_name(target_name: Option<&str>, output: &Path) -> String {
    target_name.map(str::to_string).unwrap_or_else(|| {
        output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

fn read_symbols(path: &Path) -> Result<gaia_artifact_providers::DynamicSymbols, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    dynamic_symbols(&bytes).map_err(|error| format!("'{}': {error}", path.display()))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn size(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|meta| meta.len())
        .unwrap_or_default()
}

/// Rustc's version script with every exported name not in `keep` left out;
/// the crate metadata symbol (`rust_metadata_*`) stays, since crates that
/// build against a `dylib` read it.
pub(crate) fn trimmed_script(script: &str, keep: &BTreeSet<String>) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        let entry = line.trim();
        let name = entry.strip_suffix(';').map(str::trim_end).filter(|name| {
            name.chars().next().is_some_and(|first| {
                first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '.')
            }) && !name.contains(char::is_whitespace)
                && !name.ends_with(':')
        });
        if let Some(name) = name
            && !keep.contains(name)
            && !name.starts_with("rust_metadata_")
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The recorded link's arguments pointed at the record's copies, writing
/// `output` with the version script `map`, and `--gc-sections` added.
pub(crate) fn replay_args(record: &LinkRecord, map: &Path, output: &Path) -> Vec<String> {
    let rustc_copy = record.dir.join("rustc");
    let rustc_prefix = format!("{}/", record.rustc_dir);
    let mut args = Vec::with_capacity(record.args.len() + 1);
    let mut after_output_flag = false;
    for arg in &record.args {
        let replaced = if after_output_flag {
            output.display().to_string()
        } else if arg.starts_with("-Wl,--version-script=") {
            format!("-Wl,--version-script={}", map.display())
        } else if let Some(rest) = arg.strip_prefix(&rustc_prefix) {
            rustc_copy.join(rest).display().to_string()
        } else if *arg == record.rustc_dir {
            rustc_copy.display().to_string()
        } else if arg.ends_with(".rcgu.o") {
            let file = Path::new(arg).file_name().unwrap_or_default();
            record.dir.join("objects").join(file).display().to_string()
        } else {
            arg.clone()
        };
        after_output_flag = arg == "-o";
        args.push(replaced);
    }
    args.push("-Wl,--gc-sections".into());
    args
}

/// Where the cargo build of `contract` puts its final outputs.
pub(crate) fn profile_dir(target_dir: &Path, contract: &ArtifactExecutionContract) -> PathBuf {
    crate::cargo_output_dir(target_dir, contract.artifact_target.as_deref())
        .join(crate::cargo_profile_dir(contract))
}

#[cfg(test)]
#[path = "link_trim_tests.rs"]
mod tests;
