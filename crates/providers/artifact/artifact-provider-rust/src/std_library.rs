//! The toolchain's standard library as an artifact (`package = "std"`): the
//! target's `libstd-<hash>.so`, which artifacts built with `link = "shared"`
//! need at run time. It is read from the rustc that builds the source, in the
//! artifact's execution backend, so its hash is the one those artifacts were
//! linked against.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    ProcessCancelCheck, ProcessLogLine, ProcessLogSink, ProcessLogStream, artifact_output_path,
    copy_artifact_file_to_output, run_command_with_retries,
};

use crate::tools::strip_collected_output;

/// Copies the target's `libstd-*.so` into the artifact's output directory,
/// replacing any library an earlier toolchain left there, stripped when the
/// host has a strip for the target. `target_dir` is the
/// cargo target dir, which the backend can write: the files are staged there.
pub(crate) fn collect(
    source_dir: &str,
    contract: &ArtifactExecutionContract,
    target_dir: &Path,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ArtifactProviderError> {
    let stage = stage_dir(target_dir, contract.artifact_target.as_deref());
    run_command_with_retries(
        &copy_command(source_dir, contract.artifact_target.as_deref(), &stage),
        contract,
        "copy the Rust standard library",
        log_sink.clone(),
        cancel_check,
    )?;
    let libraries = std_libraries(&stage)?;
    if libraries.is_empty() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::OutputMissing,
            format!(
                "the toolchain has no libstd-*.so for {}",
                contract.artifact_target.as_deref().unwrap_or("the host")
            ),
        ));
    }
    let output_dir = artifact_output_path(contract, source_dir);
    fs::create_dir_all(&output_dir).map_err(|error| io_error(&output_dir, error))?;
    for stale in std_libraries(&output_dir)? {
        fs::remove_file(&stale).map_err(|error| io_error(&stale, error))?;
    }
    for library in libraries {
        let Some(name) = library.file_name() else {
            continue;
        };
        let dest = output_dir.join(name);
        copy_artifact_file_to_output(&library, &dest, "Rust standard library")?;
        // The toolchain's copy carries its symbol table and debug info
        // (3.0 MB for aarch64 at 1.99, 1.2 MB stripped); the dynamic symbols
        // the programs bind to stay either way.
        match strip_collected_output(&dest, contract) {
            Err(error) if error.kind == ArtifactProviderErrorKind::PolicyBlocked => {
                if let Some(sink) = &log_sink {
                    sink(ProcessLogLine {
                        stream: ProcessLogStream::Stderr,
                        line: format!("{} unstripped: {}", dest.display(), error.message),
                    });
                }
            }
            result => result?,
        }
    }
    Ok(())
}

fn stage_dir(target_dir: &Path, target: Option<&str>) -> PathBuf {
    target_dir
        .join("gaia-rust-std")
        .join(target.unwrap_or("host"))
}

/// A shell command that empties `stage` and copies the target's
/// `libstd-*.so` into it, run from the source so its `rust-toolchain.toml`
/// picks the rustc.
fn copy_command(source_dir: &str, target: Option<&str>, stage: &Path) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(
            r#"set -e
dir=$(rustc --print target-libdir ${2:+--target "$2"})
mkdir -p "$1"
rm -f "$1"/libstd-*.so
cp "$dir"/libstd-*.so "$1"/"#,
        )
        .arg("sh")
        .arg(stage)
        .arg(target.unwrap_or_default())
        .current_dir(source_dir);
    command
}

/// The `libstd-*.so` files directly in `dir`, sorted; none when it is missing.
fn std_libraries(dir: &Path) -> Result<Vec<PathBuf>, ArtifactProviderError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(dir, error)),
    };
    let mut libraries = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| io_error(dir, error))?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.starts_with("libstd-") && name.ends_with(".so") && path.is_file() {
            libraries.push(path);
        }
    }
    libraries.sort();
    Ok(libraries)
}

fn io_error(path: &Path, error: std::io::Error) -> ArtifactProviderError {
    ArtifactProviderError::new(
        ArtifactProviderErrorKind::RuntimeState,
        format!("Rust standard library at '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_libstd_shared_objects_are_collected() {
        let temp = gaia_testkit::TempDir::new("gaia-rust-std-library");
        let scratch = temp.join("out");
        fs::create_dir_all(&scratch).expect("scratch");
        for name in [
            "libstd-abc.so",
            "libstd-abc.rlib",
            "libtest-abc.so",
            "notes",
        ] {
            fs::write(scratch.join(name), b"x").expect("file");
        }
        let found = std_libraries(&scratch).expect("list");
        assert_eq!(found, vec![scratch.join("libstd-abc.so")]);
        fs::remove_dir_all(&scratch).expect("cleanup");
        assert!(std_libraries(&scratch).expect("missing").is_empty());
    }

    #[test]
    fn the_copy_asks_the_sources_rustc_for_the_targets_libdir() {
        let command = copy_command("/src", Some("aarch64-unknown-linux-gnu"), Path::new("/t/s"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args[1].contains("rustc --print target-libdir"));
        assert_eq!(&args[2..], ["sh", "/t/s", "aarch64-unknown-linux-gnu"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/src")));
    }
}
