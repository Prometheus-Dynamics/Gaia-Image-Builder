use gaia_spec::{ByteSize, HostToolStepSpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

/// `[providers.buildroot]` work dir, RAM budget and host tool policy. These
/// settings are read only when a build runs, so mistakes are reported here.
pub(crate) fn validate_buildroot_policy(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let buildroot = &spec.policy.providers.buildroot;

    let work_dir = &buildroot.work_dir.work_dir;
    if work_dir.trim().is_empty() {
        diagnostics.push(error(
            "buildroot_work_dir_empty",
            "providers.buildroot.work_dir cannot be empty; use \"disk\", \"ram\" or a directory"
                .into(),
            Some("providers.buildroot.work_dir".into()),
        ));
    }

    if let Some(raw) = &buildroot.work_dir.ram_budget {
        let message = match raw.parse::<ByteSize>() {
            Err(parse_error) => Some(format!(
                "providers.buildroot.ram_budget '{raw}': {parse_error}"
            )),
            Ok(size) if size.bytes() == 0 => Some(format!(
                "providers.buildroot.ram_budget '{raw}' must be greater than zero"
            )),
            Ok(_) => None,
        };
        if let Some(message) = message {
            diagnostics.push(error(
                "buildroot_ram_budget_invalid",
                message,
                Some("providers.buildroot.ram_budget".into()),
            ));
        }
    }

    let host_tools = &buildroot.host_tools;
    for (key, text) in &host_tools.invalid {
        diagnostics.push(error(
            "buildroot_host_tools_invalid",
            format!(
                "providers.buildroot.host_tools.{key}: expected a comma-separated list of \
                 system, build, fail; got '{text}'"
            ),
            Some(format!("providers.buildroot.host_tools.{key}")),
        ));
    }

    let lists = std::iter::once(("default", &host_tools.default)).chain(
        host_tools
            .tools
            .iter()
            .map(|(tool, steps)| (tool.as_str(), steps)),
    );
    for (key, steps) in lists {
        if let Some(message) = unreachable_steps_message(key, steps) {
            diagnostics.push(warning(
                "buildroot_host_tools_unreachable_step",
                message,
                Some(format!("providers.buildroot.host_tools.{key}")),
            ));
        }
    }
}

/// `system` falls through to the next step when the tool is missing, but
/// `build` and `fail` always end the list, so any step after the first of
/// them is never tried.
fn unreachable_steps_message(key: &str, steps: &[HostToolStepSpec]) -> Option<String> {
    let index = steps
        .iter()
        .position(|step| matches!(step, HostToolStepSpec::Build | HostToolStepSpec::Fail))?;
    let terminal = steps[index];
    let unreachable = steps.len() - index - 1;
    if unreachable == 0 {
        return None;
    }
    let reason = match terminal {
        HostToolStepSpec::Fail => "stops the build",
        _ => "always ends the list",
    };
    Some(format!(
        "providers.buildroot.host_tools.{key}: '{}' {reason}, so the {unreachable} step(s) after it are never tried",
        terminal.as_str()
    ))
}
