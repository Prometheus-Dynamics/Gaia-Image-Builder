use gaia_spec::ResolvedBuildSpec;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::{ReportBundle, ReportFileKind, ReportOutputBundle, ReportOutputFile};
use crate::output_sizes::write_baseline;

/// The directory the run's reports are written to: `.gaia/reports` under the
/// output dir.
pub(crate) fn report_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    spec.workspace.out_path().join(".gaia").join("reports")
}

/// The file name stem of the run's reports: the build name and variant.
pub(crate) fn report_file_stem(spec: &ResolvedBuildSpec) -> String {
    spec.report_name().replace(['/', '\\', ' '], "-")
}

pub fn write_report_bundle(
    spec: &ResolvedBuildSpec,
    report: &ReportBundle,
) -> io::Result<ReportOutputBundle> {
    let report_dir = report_dir(spec);
    tracing::debug!(
        build = %spec.build_name(),
        report_dir = %report_dir.display(),
        "writing report bundle"
    );
    fs::create_dir_all(&report_dir)?;

    let build_name = report_file_stem(spec);
    let files = vec![
        write_json_file(
            &report_dir,
            &format!("{build_name}.summary.json"),
            ReportFileKind::Summary,
            &report.summary,
        )?,
        write_json_file(
            &report_dir,
            &format!("{build_name}.selection.json"),
            ReportFileKind::Selection,
            &report.selection,
        )?,
        write_json_file(
            &report_dir,
            &format!("{build_name}.provenance.json"),
            ReportFileKind::Provenance,
            &report.provenance,
        )?,
        write_json_file(
            &report_dir,
            &format!("{build_name}.manifest.json"),
            ReportFileKind::Manifest,
            &report.manifest,
        )?,
        write_json_file(
            &report_dir,
            &format!("{build_name}.rebuild-reasons.json"),
            ReportFileKind::RebuildReasons,
            &report.rebuild_reasons,
        )?,
        write_json_file(
            &report_dir,
            &format!("{build_name}.trace.json"),
            ReportFileKind::Trace,
            &crate::profile::chrome_trace(&report.summary),
        )?,
    ];

    if let Err(error) = write_baseline(spec, &report.summary) {
        tracing::warn!(
            build = %spec.build_name(),
            error = %error,
            "failed to write the image size baseline"
        );
    }
    tracing::debug!(
        build = %spec.build_name(),
        file_count = files.len(),
        "report bundle written"
    );
    Ok(ReportOutputBundle { files })
}

fn write_json_file<T: Serialize>(
    report_dir: &Path,
    filename: &str,
    kind: ReportFileKind,
    value: &T,
) -> io::Result<ReportOutputFile> {
    let path = report_dir.join(filename);
    let text = serde_json::to_string_pretty(value).map_err(|error| {
        io::Error::other(format!(
            "failed to serialize report file '{filename}': {error}"
        ))
    })?;
    // A report can quote a command's output or an error, so the secret values
    // in it are replaced before it is written.
    let bytes = gaia_process::secrets::mask(&text).into_owned().into_bytes();
    fs::write(&path, &bytes)?;
    tracing::trace!(
        kind = ?kind,
        path = %path.display(),
        bytes = bytes.len(),
        "report file written"
    );

    Ok(ReportOutputFile {
        kind,
        path,
        bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    #[test]
    fn a_report_file_replaces_a_secret_value_it_quotes() {
        let scratch = gaia_testkit::TempDir::new("gaia-report-secret");
        let dir = scratch.join("report");
        std::fs::create_dir_all(&dir).expect("report dir");
        gaia_process::secrets::install(
            vec![gaia_process::secrets::SecretDef {
                name: "gaia_test_report".into(),
                source: gaia_process::secrets::SecretSource::Command(vec![
                    "printf".into(),
                    "gaia-report-secret-88".into(),
                ]),
                required: true,
            }],
            "",
        );
        gaia_process::secrets::resolve("gaia_test_report").expect("resolve");
        let value = serde_json::json!({
            "message": "push failed with gaia-report-secret-88",
        });

        let file = write_json_file(&dir, "summary.json", ReportFileKind::Summary, &value)
            .expect("report written");

        let text = std::fs::read_to_string(&file.path).expect("read back");
        assert!(text.contains("push failed with ***"), "{text}");
        assert!(!text.contains("gaia-report-secret-88"), "{text}");
    }
}
