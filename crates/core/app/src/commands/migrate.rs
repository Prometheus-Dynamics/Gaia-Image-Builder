//! `gaia migrate <build>...`: rewrites the builds' config v2 files to v3,
//! checking each build resolves the same before and after (see
//! `gaia_config::migrate_builds`). `--check` changes nothing and fails while
//! any file is still v2, for CI.

use gaia_config::migrate_builds;

use super::CommandOutcome;

pub(crate) fn migrate_command(builds: &[String], check_only: bool) -> CommandOutcome {
    let report = match migrate_builds(builds, check_only) {
        Ok(report) => report,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let v2: Vec<String> = report
        .v2_files()
        .map(|path| path.display().to_string())
        .collect();
    if check_only {
        if v2.is_empty() {
            return CommandOutcome::Text {
                text: format!("all {} config file(s) are config v3", report.files.len()),
            };
        }
        return CommandOutcome::Failed {
            message: format!(
                "{} of {} config file(s) still use config v2 (run `gaia migrate` on the build):\n  {}",
                v2.len(),
                report.files.len(),
                v2.join("\n  ")
            ),
        };
    }
    if report.migrated.is_empty() {
        return CommandOutcome::Text {
            text: format!(
                "nothing to migrate: all {} config file(s) are config v3",
                report.files.len()
            ),
        };
    }
    let mut text = format!(
        "migrated {} config file(s) to config v3; checked that {} resolution(s) build the same:\n",
        report.migrated.len(),
        report.compared.len()
    );
    for path in &report.migrated {
        text.push_str(&format!("  {}\n", path.display()));
    }
    for compared in &report.compared {
        text.push_str(&format!("  checked {compared}\n"));
    }
    CommandOutcome::Text {
        text: text.trim_end().to_string(),
    }
}
