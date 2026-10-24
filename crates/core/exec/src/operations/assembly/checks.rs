//! `[[image.assembly.checks]]`: commands run over the finished assembly, after
//! every step. Each is a program and its arguments (no shell) with assembly
//! tokens expanded, run from the workspace root; a non-zero exit fails the
//! build with the command's output tail.

use super::*;

pub(super) fn run_assembly_checks(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    checks: &[gaia_spec::AssemblyCheckSpec],
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    messages: &mut Vec<String>,
) -> Result<(), AssemblyError> {
    for check in checks {
        let args = check
            .command
            .iter()
            .map(|arg| roots.resolve_text(spec, arg))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("assembly check '{}': {error}", check.id))?;
        let Some((program, rest)) = args.split_first() else {
            return Err(format!("assembly check '{}' has no command", check.id).into());
        };
        let mut command = Command::new(program);
        command.args(rest).current_dir(&spec.workspace.root_dir);
        let timeout = check
            .timeout_secs
            .map(|seconds| Duration::from_secs(seconds.max(1)))
            .unwrap_or_else(|| assembly_command_timeout(spec));
        let result = gaia_process::run_command_with_timeout_and_retention(
            &mut command,
            timeout,
            "assembly check",
            process_output_retention(spec),
            None,
            cancel_check.clone(),
        )
        .map_err(|error| AssemblyError::process(&command, error))?;
        let output = CommandCapturedOutput {
            status: result.output.status,
            stdout: result.output.stdout,
            stderr: result.output.stderr,
        };
        if !output.status.success() {
            return Err(format!(
                "assembly check '{}' failed: {}",
                check.id,
                output.failure_context(&command)
            )
            .into());
        }
        messages.push(format!("assembly check '{}' passed", check.id));
        for line in output
            .stdout_tail()
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            messages.push(format!("assembly check '{}': {line}", check.id));
        }
    }
    Ok(())
}
