//! The report as text, for a person to read. The JSON is the report; this
//! is the same content laid out.

use crate::model::{FORMAT, Report};
use std::fmt::Write;

/// The report as aligned text, with the delta when there is one.
pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{FORMAT} v{}: image {}", report.version, report.image);
    let _ = writeln!(
        out,
        "image     {} ({} bytes)",
        report.image_object, report.image_bytes
    );
    let _ = writeln!(
        out,
        "files     {} in {} components, {} bytes",
        report.files.len(),
        report.components.len(),
        report.total_bytes
    );
    let _ = writeln!(
        out,
        "previous  {}",
        report.previous.as_deref().unwrap_or("none")
    );

    let _ = writeln!(out, "\nsources");
    if report.sources.is_empty() {
        let _ = writeln!(out, "  none in the lock");
    }
    for s in &report.sources {
        let _ = writeln!(
            out,
            "  {:<12} {:<6} {:<8} {}",
            format!("{}.{}", s.owner, s.source),
            s.kind,
            short(&s.resolved),
            s.locator
        );
    }

    let _ = writeln!(out, "\ncomponents");
    let _ = writeln!(out, "  {:<12} {:>6} {:>12}", "owner", "files", "bytes");
    for c in &report.components {
        let _ = writeln!(out, "  {:<12} {:>6} {:>12}", c.owner, c.files, c.bytes);
    }

    let _ = writeln!(out, "\nfiles");
    let _ = writeln!(
        out,
        "  {:<40} {:<12} {:<6} {:>10}",
        "path", "owner", "mode", "bytes"
    );
    for f in &report.files {
        let _ = writeln!(
            out,
            "  {:<40} {:<12} {:<6} {:>10}",
            f.path,
            f.owners.join("+"),
            f.mode,
            f.bytes
        );
    }

    if let Some(delta) = &report.delta {
        let _ = writeln!(
            out,
            "\ndelta against {}",
            report.previous.as_deref().unwrap_or("")
        );
        let _ = writeln!(
            out,
            "  image {:+} bytes, total {:+} bytes",
            delta.image_change, delta.total_change
        );
        for c in &delta.components {
            let _ = writeln!(
                out,
                "  component {:<12} {} -> {} bytes",
                c.owner, c.before, c.after
            );
        }
        for s in &delta.sources {
            let _ = writeln!(
                out,
                "  source {:<12} {} -> {}",
                s.name,
                s.before.as_deref().map_or("none".into(), short),
                s.after.as_deref().map_or("none".into(), short)
            );
        }
        for path in &delta.added {
            let _ = writeln!(out, "  added    {path}");
        }
        for path in &delta.removed {
            let _ = writeln!(out, "  removed  {path}");
        }
        for c in &delta.changed {
            let _ = writeln!(
                out,
                "  changed  {}  {} -> {} bytes, mode {} -> {}, owners {} -> {}",
                c.path,
                c.before_bytes,
                c.after_bytes,
                c.before_mode,
                c.after_mode,
                c.before_owners.join("+"),
                c.after_owners.join("+")
            );
        }
        if delta.added.is_empty()
            && delta.removed.is_empty()
            && delta.changed.is_empty()
            && delta.sources.is_empty()
            && delta.components.is_empty()
            && delta.image_change == 0
        {
            let _ = writeln!(out, "  no change");
        }
    }
    out
}

/// A commit or digest, shortened for a column.
fn short(resolved: &str) -> String {
    let tail = resolved.rsplit(':').next().unwrap_or(resolved);
    tail.chars().take(12).collect()
}
