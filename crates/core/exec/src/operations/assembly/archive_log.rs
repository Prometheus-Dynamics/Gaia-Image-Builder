//! The step time and size lines of the archives the assembly writes
//! (compressed raw disks, gzip/zstd transforms, tar archives), so a run
//! shows what each archive cost and how well it compressed.

use std::path::Path;
use std::time::Duration;

/// The step time and size lines of a finished archive: `archive <name>
/// (<label>)` with its wall time, and `archived <name>: <in> -> <out> in
/// <secs>s` (just the output size when the input is not known).
pub(super) fn archive_log_messages(
    output: &Path,
    label: &str,
    input_bytes: Option<u64>,
    elapsed: Duration,
) -> Vec<String> {
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| output.display().to_string());
    let output_bytes = std::fs::metadata(output).map_or(0, |metadata| metadata.len());
    let sizes = match input_bytes {
        Some(input) => format!("{} -> {}", format_bytes(input), format_bytes(output_bytes)),
        None => format_bytes(output_bytes),
    };
    vec![
        gaia_process::step_time_message(&format!("archive {name} ({label})"), elapsed),
        format!("archived {name}: {sizes} in {}s", elapsed.as_secs()),
    ]
}

/// `1.4 GiB`, `142 MiB`, `812 B`.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value >= 100.0 => format!("{value:.0} {}", UNITS[unit]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}
