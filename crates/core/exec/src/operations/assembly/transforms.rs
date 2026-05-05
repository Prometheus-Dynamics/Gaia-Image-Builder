use super::*;

pub(super) struct AssemblyTransformSummary {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) tool_path: Option<String>,
    pub(super) tool_version: Option<String>,
}

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
            publish_assembly_output(&temp, &dest)?;
            Ok(AssemblyTransformSummary {
                src,
                bytes: file_len(&dest)?,
                sha256: file_sha256(&dest)?,
                dest,
                tool_path: None,
                tool_version: None,
            })
        }
        gaia_spec::AssemblyTransformKindSpec::Gzip => {
            let tool = resolve_assembly_tool(roots, "gzip")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&dest);
            let mut command = Command::new(&tool.program);
            command.arg("-n").arg("-c").arg(&src);
            let output = run_command_stdout_to_file(
                spec,
                &mut command,
                &temp,
                process_output_retention(spec),
                cancel_check,
            )?;
            if !output.status.success() {
                return Err(format!(
                    "gzip transform failed for '{}' using '{}': {}",
                    src.display(),
                    tool.display,
                    output.failure_context(&command)
                )
                .into());
            }
            publish_assembly_output(&temp, &dest)?;
            Ok(AssemblyTransformSummary {
                src,
                bytes: file_len(&dest)?,
                sha256: file_sha256(&dest)?,
                dest,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
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
            publish_assembly_output(&temp, &dest)?;
            Ok(AssemblyTransformSummary {
                src,
                bytes: file_len(&dest)?,
                sha256: file_sha256(&dest)?,
                dest,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
    }
}
