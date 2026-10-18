use super::*;

pub(super) struct AssemblyTransformSummary {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) tool_path: Option<String>,
    pub(super) tool_version: Option<String>,
}

/// The zstd level of a `zstd` transform when the spec sets none: the same
/// as `cpio-zstd`.
const ZSTD_TRANSFORM_DEFAULT_LEVEL: u32 = 19;
/// `--long` is only worth its window for inputs past this size.
const ZSTD_LONG_MIN_INPUT_BYTES: u64 = 128 * 1024 * 1024;
/// The long-distance window `--long` asks for: 2^27 bytes.
const ZSTD_LONG_OPTION: &str = "--long=27";

pub(super) fn execute_assembly_transform(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    transform: &gaia_spec::AssemblyTransformSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyTransformSummary, AssemblyError> {
    let src = transform
        .src
        .as_ref()
        .ok_or_else(|| {
            format!(
                "assembly transform '{}' requires src",
                transform.kind.as_str()
            )
        })
        .and_then(|src| roots.resolve_path(spec, src))?;
    if !src.is_file() {
        return Err(format!(
            "assembly transform source '{}' does not exist or is not a file",
            src.display()
        )
        .into());
    }
    let dest = roots.resolve_path(spec, &transform.dest)?;
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly transform destination dir '{}': {error}",
                parent.display()
            )
        })?;
    }

    match transform.kind {
        gaia_spec::AssemblyTransformKindSpec::Copy => {
            let temp = temporary_assembly_output_path(&dest);
            std_fs::copy(&src, &temp).map_err(|error| {
                format!(
                    "failed to copy assembly transform '{}' to '{}': {error}",
                    src.display(),
                    temp.display()
                )
            })?;
            let sha256 = publish_assembly_output(&temp, &dest)?;
            Ok(AssemblyTransformSummary {
                src,
                bytes: file_len(&dest)?,
                sha256,
                dest,
                tool_path: None,
                tool_version: None,
            })
        }
        gaia_spec::AssemblyTransformKindSpec::Gzip => {
            // `-n` leaves the name and the modification time out of the
            // header, so the same input and level give the same bytes.
            let level = transform.level.map(|level| format!("-{level}"));
            let mut args = vec!["-n", "-c"];
            args.extend(level.as_deref());
            compress_to_dest(spec, roots, "gzip", &args, src, dest, cancel_check)
        }
        gaia_spec::AssemblyTransformKindSpec::Zstd => {
            let input_bytes = std_fs::metadata(&src).map_or(0, |metadata| metadata.len());
            let args = zstd_transform_args(transform.level, transform.long, input_bytes);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            compress_to_dest(spec, roots, "zstd", &args, src, dest, cancel_check)
        }
        gaia_spec::AssemblyTransformKindSpec::Xz => {
            // The block size is fixed, so the output does not depend on the
            // thread count (the same flags as the raw disk archives).
            let level = transform.level.map(|level| format!("-{level}"));
            let mut args = vec!["-T0", "--block-size=24MiB", "-c"];
            args.extend(level.as_deref());
            compress_to_dest(spec, roots, "xz", &args, src, dest, cancel_check)
        }
        gaia_spec::AssemblyTransformKindSpec::Verity => Err(format!(
            "assembly transform 'verity' for '{}' runs as a verity step, not a copy",
            dest.display()
        )
        .into()),
        gaia_spec::AssemblyTransformKindSpec::CompileDts => {
            let tool = resolve_assembly_tool(roots, "dtc")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&dest);
            let mut command = Command::new(&tool.program);
            command
                .arg("-@")
                .arg("-I")
                .arg("dts")
                .arg("-O")
                .arg("dtb")
                .arg("-o")
                .arg(&temp)
                .arg(&src);
            let output = run_command_capture_tail(
                spec,
                &mut command,
                process_output_retention(spec),
                cancel_check,
            )?;
            if !output.status.success() {
                let _ = std_fs::remove_file(&temp);
                return Err(format!(
                    "compile-dts transform failed for '{}' using '{}': {}",
                    src.display(),
                    tool.display,
                    output.failure_context(&command)
                )
                .into());
            }
            let sha256 = publish_assembly_output(&temp, &dest)?;
            Ok(AssemblyTransformSummary {
                src,
                bytes: file_len(&dest)?,
                sha256,
                dest,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
    }
}

/// The zstd arguments of a `zstd` transform: level 19 unless the spec sets
/// one, the same default as `cpio-zstd`. The level is fixed, so the output is
/// the same for any thread count. `--long` only where the window matters and
/// only when asked (`long = true`): the default output decodes with a stock
/// `zstd -d`.
pub(super) fn zstd_transform_args(level: Option<u32>, long: bool, input_bytes: u64) -> Vec<String> {
    let level = level.unwrap_or(ZSTD_TRANSFORM_DEFAULT_LEVEL);
    let mut args = vec![
        "-q".to_string(),
        "-c".to_string(),
        "--no-progress".to_string(),
        "-T0".to_string(),
        format!("-{level}"),
    ];
    if long && level >= ZSTD_TRANSFORM_DEFAULT_LEVEL && input_bytes > ZSTD_LONG_MIN_INPUT_BYTES {
        args.push(ZSTD_LONG_OPTION.to_string());
    }
    args
}

/// Runs `<tool> <args> <src>` with stdout published to `dest`.
fn compress_to_dest(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    tool_name: &str,
    args: &[&str],
    src: PathBuf,
    dest: PathBuf,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyTransformSummary, AssemblyError> {
    let tool = resolve_assembly_tool(roots, tool_name)?;
    tracing::Span::current().record("tool_path", tool.display.as_str());
    let temp = temporary_assembly_output_path(&dest);
    let mut command = Command::new(&tool.program);
    command.args(args).arg(&src);
    let output = run_command_stdout_to_file(
        spec,
        &mut command,
        &temp,
        process_output_retention(spec),
        cancel_check,
    )?;
    if !output.status.success() {
        let _ = std_fs::remove_file(&temp);
        return Err(format!(
            "{tool_name} transform failed for '{}' using '{}': {}",
            src.display(),
            tool.display,
            output.failure_context(&command)
        )
        .into());
    }
    let sha256 = publish_assembly_output(&temp, &dest)?;
    Ok(AssemblyTransformSummary {
        src,
        bytes: file_len(&dest)?,
        sha256,
        dest,
        tool_path: Some(tool.display.clone()),
        tool_version: tool_version(&tool, ["--version"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_transform_defaults_to_level_19_and_long_is_opt_in() {
        let big = ZSTD_LONG_MIN_INPUT_BYTES + 1;
        assert_eq!(
            zstd_transform_args(None, false, big),
            ["-q", "-c", "--no-progress", "-T0", "-19"]
        );
        assert_eq!(
            zstd_transform_args(Some(3), false, big).last().unwrap(),
            "-3"
        );
        // `long = true` adds the window for level 19 over 128 MiB only.
        assert_eq!(
            zstd_transform_args(None, true, big).last().unwrap(),
            "--long=27"
        );
        assert!(
            !zstd_transform_args(None, true, ZSTD_LONG_MIN_INPUT_BYTES)
                .contains(&ZSTD_LONG_OPTION.to_string())
        );
        assert!(!zstd_transform_args(Some(18), true, big).contains(&ZSTD_LONG_OPTION.to_string()));
        assert!(!zstd_transform_args(Some(19), false, big).contains(&ZSTD_LONG_OPTION.to_string()));
    }
}
