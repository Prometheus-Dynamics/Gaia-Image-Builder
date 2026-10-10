//! Writing an image execution's outputs: the collect dir's marker and state
//! files, the image archive, and the content digests recorded for them (see
//! `content_digests`).

use crate::content_digests::{self, collect_content_files, sha256_hex};
use crate::{
    ImageExecutionResult, ImageProviderError, dir_digest, file_sha256_or_placeholder, path_bytes,
};
use std::fs;
use std::path::Path;

pub fn materialize_image_output(result: &ImageExecutionResult) -> Result<(), ImageProviderError> {
    // One hash of the archive serves both the state's archive_sha256 and the
    // content manifest.
    let archive_digest = result
        .archive_path
        .as_ref()
        .filter(|path| path.is_file())
        .and_then(|path| sha256_hex(path).ok());
    write_image_output(result, archive_digest.as_deref())?;
    record_image_output_digests(result, archive_digest);
    Ok(())
}

/// Records the content digests of the collect dir and archive, so the next
/// plan reads them from the manifest instead of hashing the files again.
/// Best effort: a missing entry only costs a re-hash.
fn record_image_output_digests(result: &ImageExecutionResult, archive_digest: Option<String>) {
    let archive = result.archive_path.as_deref().filter(|path| path.is_file());
    if let Some(collect_dir) = &result.collect_dir {
        let items = collect_content_files(collect_dir)
            .into_iter()
            .map(|path| {
                let digest = archive
                    .filter(|archive| *archive == path.as_path())
                    .and(archive_digest.clone());
                (path, digest)
            })
            .collect();
        let _ = content_digests::record_content_entries(collect_dir, items);
    }
    if let (Some(archive), Some(parent)) = (archive, archive.and_then(Path::parent)) {
        let _ = content_digests::record_content_entries(
            parent,
            vec![(archive.to_path_buf(), archive_digest)],
        );
    }
}

fn write_image_output(
    result: &ImageExecutionResult,
    archive_digest: Option<&str>,
) -> Result<(), ImageProviderError> {
    if let Some(collect_dir) = &result.collect_dir {
        fs::create_dir_all(collect_dir)
            .map_err(|error| {
                format!(
                    "failed to create image collect dir '{}': {error}",
                    collect_dir.display()
                )
            })
            .map_err(ImageProviderError::backend_command)?;
        let marker = collect_dir.join("image-provider.txt");
        fs::write(
            &marker,
            format!(
                "provider={}\nemit_report={}\n",
                result.provider_id, result.emit_report
            ),
        )
        .map_err(|error| {
            format!(
                "failed to write image marker '{}': {error}",
                marker.display()
            )
        })
        .map_err(ImageProviderError::runtime_state)?;
        let state_path = collect_dir.join(".gaia-image-state.txt");
        fs::write(&state_path, render_image_state(result, archive_digest))
            .map_err(|error| {
                format!(
                    "failed to write image state '{}': {error}",
                    state_path.display()
                )
            })
            .map_err(ImageProviderError::backend_command)?;
    }
    if let Some(archive_path) = &result.archive_path {
        if archive_path.is_file() {
            return Ok(());
        }
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| {
                    format!(
                        "failed to create archive dir '{}': {error}",
                        parent.display()
                    )
                })
                .map_err(ImageProviderError::backend_command)?;
        }
        let temp_archive = archive_path.with_extension("gaia.tmp");
        fs::write(
            &temp_archive,
            format!("provider={}\narchive=true\n", result.provider_id),
        )
        .map_err(|error| {
            format!(
                "failed to write image temp archive '{}': {error}",
                temp_archive.display()
            )
        })
        .map_err(ImageProviderError::backend_command)?;
        finalize_temp_image_output(&temp_archive, archive_path, "image archive")?;
    }
    Ok(())
}

pub fn finalize_temp_image_output(
    temp_output: &Path,
    output_path: &Path,
    label: &str,
) -> Result<(), ImageProviderError> {
    fs::rename(temp_output, output_path).map_err(|error| {
        let _ = fs::remove_file(temp_output);
        ImageProviderError::backend_command(format!(
            "failed to move {label} '{}' into place '{}': {error}",
            temp_output.display(),
            output_path.display()
        ))
    })
}

fn render_image_state(result: &ImageExecutionResult, archive_digest: Option<&str>) -> String {
    let mut state = gaia_spec::KeyValueState::new()
        .with("provider", result.provider_id.as_str())
        .with("emit_report", result.emit_report)
        .with(
            "archive",
            result
                .archive_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )
        .with("reused", result.reused);
    for (index, detail) in result.reuse_details.iter().enumerate() {
        state.insert(format!("reuse_detail_{index}"), detail);
    }
    if let Some(collect_dir) = &result.collect_dir {
        state.insert("collect_digest", dir_digest(collect_dir));
    }
    if let Some(archive_path) = &result.archive_path {
        let digest = archive_digest
            .map(str::to_string)
            .unwrap_or_else(|| file_sha256_or_placeholder(archive_path));
        state.insert("archive_sha256", digest);
        state.insert("archive_bytes", path_bytes(archive_path));
    }
    state.extend_pairs(result.state_details.iter().cloned());
    state.render()
}
