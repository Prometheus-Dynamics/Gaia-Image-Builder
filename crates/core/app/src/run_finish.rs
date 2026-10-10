//! The lines `gaia run` prints after its outcome: the export of the primary
//! image (`--export`), and on a failed or cancelled run the `resume:` hint
//! that repeats the invocation.

use std::path::Path;

use gaia_report::ReportBundle;

use crate::export::{ExportedFile, export_images, primary_images, run_stamp};
use crate::{AppArgs, AppCommand, CommandOutcome};

/// What to print after a `gaia run` outcome.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunFinish {
    /// Printed in order, after the outcome itself; the last line is the
    /// resume hint when the run failed or was cancelled.
    pub lines: Vec<String>,
    /// True when an export was asked for (`--export` or a configured
    /// directory) and copying failed; the process then exits 1.
    pub export_failed: bool,
}

/// The directory this run exports to. `--no-export` turns exports off for
/// the run; `--export <dir>` beats the configured directory, which is the
/// build's `image.output.export_dir` before the workspace's `workspace.export_dir`.
fn export_dir_for(args: &AppArgs, outcome: &CommandOutcome) -> Option<String> {
    if args.no_export {
        return None;
    }
    if let Some(export_dir) = &args.export_dir {
        return Some(export_dir.clone());
    }
    match outcome {
        CommandOutcome::Ran {
            configured_export_dir,
            ..
        } => configured_export_dir.clone(),
        _ => None,
    }
}

/// The lines for the outcome of `args`. Only `gaia run` has any: exports
/// happen after a successful run, and a failed or cancelled run gets the
/// resume hint. Exports are copied here, so calling this has side effects.
pub fn run_finish(args: &AppArgs, outcome: &CommandOutcome) -> RunFinish {
    let mut finish = RunFinish::default();
    if args.command != AppCommand::Run {
        return finish;
    }
    let report = match outcome {
        CommandOutcome::Ran { report, .. } => Some(report),
        _ => None,
    };
    let succeeded = report.is_some() && outcome.exit_code() == 0;

    if let Some(export_dir) = export_dir_for(args, outcome) {
        match (succeeded, report) {
            (true, Some(report)) => {
                let (lines, failed) = export_lines(report, &export_dir);
                finish.lines.extend(lines);
                finish.export_failed = failed;
            }
            _ => finish
                .lines
                .push("export: skipped, the run did not succeed; nothing was exported".into()),
        }
    }
    if let Some(report) = report
        && !succeeded
    {
        finish.lines.push(resume_line(args, report));
    }
    finish
}

fn export_lines(report: &ReportBundle, export_dir: &str) -> (Vec<String>, bool) {
    let Some(primary) = report.summary.primary_image_output.as_deref() else {
        return (
            vec!["export: nothing to export, the run produced no image output".into()],
            false,
        );
    };
    let sources = match primary_images(Path::new(primary)) {
        Ok(sources) => sources,
        Err(error) => return (vec![format!("export failed: {error}")], true),
    };
    if sources.is_empty() {
        return (
            vec![format!(
                "export: nothing to export, no disk image in '{primary}'"
            )],
            false,
        );
    }
    let exported = export_images(
        &sources,
        Path::new(export_dir),
        &report.summary.build_name.replace(['/', '\\', ' '], "-"),
        report.summary.build_version.as_deref(),
        &run_stamp(),
    );
    match exported {
        Ok(files) => (files.iter().map(exported_line).collect(), false),
        Err(error) => (vec![format!("export failed: {error}")], true),
    }
}

fn exported_line(file: &ExportedFile) -> String {
    format!(
        "exported: {}  sha256 {}  {} bytes",
        file.path.display(),
        file.sha256,
        file.bytes
    )
}

/// `resume: gaia run <build> <overrides>  # ...`: the invocation that picks
/// the same run back up. Secret values come out masked as the report shows
/// them, so the hint says to enter those again.
fn resume_line(args: &AppArgs, report: &ReportBundle) -> String {
    let env = report.provenance.selected_env_overrides.as_slice();
    let set = report.provenance.explicit_overrides.as_slice();
    let (command, masked) = resume_command(args, env, set);
    let note = if report.summary.rolled_back_operations > 0 {
        "operations unwound by rollback run again"
    } else {
        "finished work is reused"
    };
    let masked_note = if masked {
        "; *** values were masked, enter them again"
    } else {
        ""
    };
    format!("resume: {command}  # {note}{masked_note}")
}

/// The `gaia run` words for `args`, with each override value replaced by the
/// report's shown value when it differs. Returns the command and whether any
/// value was masked.
fn resume_command(
    args: &AppArgs,
    shown_env: &[(String, String)],
    shown_set: &[(String, String)],
) -> (String, bool) {
    let mut masked = false;
    let mut words = vec!["gaia".to_string(), "run".into(), shell_quote(&args.build)];
    if let Some(preset) = &args.preset {
        words.extend(["--preset".into(), shell_quote(preset)]);
    }
    for path in &args.env_files {
        words.extend(["--env-file".into(), shell_quote(path)]);
    }
    for (key, value) in &args.env_overrides {
        let pair = shown_pair(key, value, shown_env, &mut masked);
        words.extend(["--env".into(), shell_quote(&pair)]);
    }
    for (key, value) in &args.explicit_overrides {
        let pair = shown_pair(key, value, shown_set, &mut masked);
        words.extend(["--set".into(), shell_quote(&pair)]);
    }
    if !args.only.is_empty() {
        words.extend(["--only".into(), shell_quote(&args.only.join(","))]);
    }
    // The resumed run exports as this one would have.
    if let Some(dir) = &args.export_dir {
        words.extend(["--export".into(), shell_quote(dir)]);
    }
    if args.no_export {
        words.push("--no-export".into());
    }
    (words.join(" "), masked)
}

fn shown_pair(key: &str, value: &str, shown: &[(String, String)], masked: &mut bool) -> String {
    let shown_value = shown
        .iter()
        .find(|(shown_key, _)| shown_key == key)
        .map(|(_, shown_value)| shown_value.as_str())
        .unwrap_or(value);
    if shown_value != value {
        *masked = true;
    }
    format!("{key}={shown_value}")
}

/// Quotes a word for a POSIX shell when it has characters beyond the safe set.
pub(crate) fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=,+@%".contains(&byte));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> AppArgs {
        AppArgs::parse_from(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn resume_command_repeats_build_and_overrides_in_order() {
        let args = parse(&[
            "run",
            "builds/heli.toml",
            "--preset",
            "ci",
            "--env-file",
            "env/ci.env",
            "--env",
            "GAIA_MODE=ci",
            "--set",
            "image.defconfig=raze_defconfig",
            "--set",
            "build.version=9.9.9",
            "--only",
            "image,install",
        ]);
        let (command, masked) = resume_command(&args, &[], &[]);
        assert_eq!(
            command,
            "gaia run builds/heli.toml --preset ci --env-file env/ci.env --env GAIA_MODE=ci \
             --set image.defconfig=raze_defconfig --set build.version=9.9.9 --only image,install"
        );
        assert!(!masked);
    }

    #[test]
    fn resume_command_does_not_repeat_rebuild_requests() {
        let args = parse(&[
            "run",
            "b.toml",
            "--only",
            "image",
            "--rebuild",
            "artifact:*",
            "--rebuild-package",
            "libfoo",
        ]);
        let (command, _) = resume_command(&args, &[], &[]);
        assert_eq!(command, "gaia run b.toml --only image");
    }

    #[test]
    fn resume_command_repeats_export_flags() {
        let mut args = AppArgs::parse_from(["run", "b.toml", "--export", "out dir"]);
        assert!(
            resume_command(&args, &[], &[])
                .0
                .ends_with("--export 'out dir'")
        );
        args = AppArgs::parse_from(["run", "b.toml", "--no-export"]);
        assert!(resume_command(&args, &[], &[]).0.ends_with("--no-export"));
    }

    #[test]
    fn resume_command_quotes_words_and_shows_masked_values() {
        let args = parse(&[
            "run",
            "my builds/heli.toml",
            "--env",
            "API_TOKEN=super-secret",
            "--set",
            "label=it's here",
        ]);
        let shown_env = vec![("API_TOKEN".to_string(), "***".to_string())];
        let (command, masked) = resume_command(&args, &shown_env, &[]);
        assert_eq!(
            command,
            "gaia run 'my builds/heli.toml' --env 'API_TOKEN=***' --set 'label=it'\\''s here'"
        );
        assert!(masked);
        assert_eq!(shell_quote("plain-1.2"), "plain-1.2");
        assert_eq!(shell_quote(""), "''");
    }
}
