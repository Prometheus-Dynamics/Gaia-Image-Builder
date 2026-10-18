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
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        io::Error::other(format!(
            "failed to serialize report file '{filename}': {error}"
        ))
    })?;
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
