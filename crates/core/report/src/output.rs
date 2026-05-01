use gaia_spec::ResolvedBuildSpec;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::Path;

use crate::model::{ReportBundle, ReportFileKind, ReportOutputBundle, ReportOutputFile};

pub fn write_report_bundle(
    spec: &ResolvedBuildSpec,
    report: &ReportBundle,
) -> io::Result<ReportOutputBundle> {
    let report_dir = spec.workspace.out_path().join(".gaia").join("reports");
    tracing::debug!(
        build = %spec.build_name(),
        report_dir = %report_dir.display(),
        "writing report bundle"
    );
    fs::create_dir_all(&report_dir)?;

    let build_name = spec.build_name().replace(['/', '\\', ' '], "-");
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
    ];

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
