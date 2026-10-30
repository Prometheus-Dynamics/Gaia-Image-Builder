//! Disk space: what a filesystem has free, and byte counts for messages.

use std::path::Path;
use std::process::{Command, Stdio};

/// Free bytes on the filesystem holding `dir` (`df -Pk`).
pub fn filesystem_available_bytes(dir: &Path) -> Option<u64> {
    let output = Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let available_kib = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()?;
    Some(available_kib * 1024)
}

/// A byte count in binary units, one decimal above bytes: `812 B`,
/// `3.4 MiB`, `142.0 MiB`, `1.2 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_shown_in_binary_units() {
        assert_eq!(format_bytes(812), "812 B");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 400 * 1024), "3.4 MiB");
        assert_eq!(format_bytes(142 * 1024 * 1024), "142.0 MiB");
        assert_eq!(format_bytes(1288490189), "1.2 GiB");
    }

    #[test]
    fn the_root_filesystem_has_a_size() {
        assert!(filesystem_available_bytes(Path::new("/")).is_some());
    }
}
