//! Rendering of a `gaia preview` report: the text printed to the terminal and
//! the JSON for scripts. The report is built in `preview.rs`.

use super::*;

pub(crate) fn print_preview(report: &PreviewReport) {
    if report.json {
        println!("{}", preview_json(report));
        return;
    }
    println!(
        "preview of '{}': {} operation(s), {} would execute",
        report.build_name,
        report.operations.len(),
        report.operations.iter().filter(|op| op.executes).count()
    );
    println!("preview: {}", report.invalidation_line());
    for operation in &report.operations {
        let state = if operation.executes { "run  " } else { "reuse" };
        println!("{state} {}: {}", operation.id, operation.reason);
    }
    for image in &report.images {
        println!();
        println!(
            "image {} ({}):",
            if image.provider_id.is_empty() {
                "-"
            } else {
                image.provider_id.as_str()
            },
            image.operations.join(", ")
        );
        if let Some(note) = &image.note {
            println!("  {note}");
        }
        let Some(preview) = &image.preview else {
            continue;
        };
        if let Some(reason) = &preview.blocked {
            println!("  blocked: {reason}");
        }
        for section in &preview.sections {
            println!("  {}:", section.title);
            for line in &section.lines {
                println!("    {line}");
            }
        }
        let outside = preview.deletions_outside_trash();
        println!(
            "  deletions: {outside} outside trash, {} in trash",
            preview.deletions.len() - outside
        );
        for deletion in preview.deletions.iter().take(LISTED_DELETIONS) {
            println!(
                "    {:<8} {}  ({})",
                deletion.kind.as_str(),
                deletion.path,
                deletion.reason
            );
        }
        if preview.deletions.len() > LISTED_DELETIONS {
            println!(
                "    ... and {} more (--json lists every path)",
                preview.deletions.len() - LISTED_DELETIONS
            );
        }
    }
    println!();
    println!("preview: {}", report.verdict());
}

/// The report as JSON, for scripts.
pub(crate) fn preview_json(report: &PreviewReport) -> serde_json::Value {
    serde_json::json!({
        "build": report.build_name,
        "verdict": report.verdict(),
        "invalidation": report.invalidation.as_ref().map(|summary| serde_json::json!({
            "line": report.invalidation_line(),
            "direct": summary.direct,
            "cascaded": summary.cascaded,
        })),
        "fail_on_clean": report.fail_on_clean,
        "trips_fail_on_clean": report.tripped(),
        "operations": report.operations.iter().map(|operation| serde_json::json!({
            "id": operation.id,
            "executes": operation.executes,
            "reason": operation.reason,
        })).collect::<Vec<_>>(),
        "images": report.images.iter().map(|image| serde_json::json!({
            "provider": image.provider_id,
            "operations": image.operations,
            "note": image.note,
            "blocked": image.preview.as_ref().and_then(|preview| preview.blocked.clone()),
            "verdict": image.preview.as_ref().map(|preview| preview.verdict.clone()),
            "clean": image.preview.as_ref().map(|preview| preview.clean.as_str()),
            "clean_reasons": image.preview.as_ref().map(|preview| preview.clean_reasons.clone()),
            "rebuilt_packages": image.preview.as_ref().map(|preview| preview.rebuilt_packages.clone()),
            "uninstalled_packages": image.preview.as_ref().map(|preview| preview.uninstalled_packages.clone()),
            "sections": image.preview.as_ref().map(|preview| preview.sections.iter().map(|section| serde_json::json!({
                "title": section.title,
                "lines": section.lines,
            })).collect::<Vec<_>>()),
            "deletions_outside_trash": image.preview.as_ref().map(ImagePreview::deletions_outside_trash),
            "deletions": image.preview.as_ref().map(|preview| preview.deletions.iter().map(|deletion| serde_json::json!({
                "kind": deletion.kind.as_str(),
                "path": deletion.path,
                "reason": deletion.reason,
            })).collect::<Vec<_>>()),
            "trips_fail_on_clean": image.preview.as_ref().map(ImagePreview::trips_fail_on_clean),
        })).collect::<Vec<_>>(),
    })
}
