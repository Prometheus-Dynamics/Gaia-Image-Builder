//! The image lines of a `gaia run` report: the primary image output with its
//! digest, then a size table of every image output the run published, with
//! its change since the previous successful run.

use std::path::Path;

use gaia_report::{OutputSizeRecord, RunSummary, SizeChange};

use crate::export::sha256_file;
use crate::output::format_bytes;

pub(crate) fn print_image_outputs(summary: &RunSummary) {
    if let Some(primary_image_output) = &summary.primary_image_output {
        println!("primary image output: {primary_image_output}");
        if let Ok(sha256) = sha256_file(Path::new(primary_image_output)) {
            println!("primary image sha256: {sha256}");
        }
    }
    if !summary.sizes.is_empty() {
        println!("image sizes (change since the previous successful run):");
        for line in output_size_lines(&summary.sizes) {
            println!("{line}");
        }
    }
}

/// One line per image output, aligned: kind, name, size and change, such as
/// `  filesystem  rootfs.erofs  182.4 MiB  (+1.3 MiB)`. The change is `new`,
/// `=` when unchanged, `(+x)` or `(-x)`, or `removed` with the old size. The
/// records are in the order given (the summary sorts them by kind then name).
pub(crate) fn output_size_lines(sizes: &[OutputSizeRecord]) -> Vec<String> {
    let kind_width = sizes
        .iter()
        .map(|record| record.kind.len())
        .max()
        .unwrap_or(0);
    let name_width = sizes
        .iter()
        .map(|record| record.name.chars().count())
        .max()
        .unwrap_or(0);
    let size_width = sizes
        .iter()
        .map(|record| size_text(record).len())
        .max()
        .unwrap_or(0);
    sizes
        .iter()
        .map(|record| {
            format!(
                "  {:<kind_width$}  {:<name_width$}  {:>size_width$}  {}",
                record.kind,
                record.name,
                size_text(record),
                size_change_text(record),
            )
        })
        .collect()
}

/// The size column: the file's size, or `-` for a removed output.
fn size_text(record: &OutputSizeRecord) -> String {
    match record.change {
        SizeChange::Removed => "-".to_string(),
        _ => format_bytes(record.bytes),
    }
}

/// The change column: `new`, `=`, `(+1.3 MiB)`, `(-12.0 KiB)` or
/// `removed (was 4.1 MiB)`.
pub(crate) fn size_change_text(record: &OutputSizeRecord) -> String {
    match record.change {
        SizeChange::New => "new".to_string(),
        SizeChange::Unchanged => "=".to_string(),
        SizeChange::Changed { .. } => {
            let delta = record.delta_bytes().unwrap_or(0);
            let sign = if delta >= 0 { '+' } else { '-' };
            format!("({sign}{})", format_bytes(delta.unsigned_abs()))
        }
        SizeChange::Removed => format!("removed (was {})", format_bytes(record.bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn record(name: &str, kind: &str, bytes: u64, change: SizeChange) -> OutputSizeRecord {
        OutputSizeRecord {
            kind: kind.into(),
            name: name.into(),
            path: format!("/out/{name}"),
            bytes,
            change,
        }
    }

    #[test]
    fn change_text_shows_grow_shrink_equal_new_and_removed() {
        let grew = record(
            "rootfs.erofs",
            "filesystem",
            182 * MIB,
            SizeChange::Changed {
                previous_bytes: 181 * MIB,
            },
        );
        assert_eq!(size_change_text(&grew), "(+1.0 MiB)");
        let shrank = record(
            "initramfs.cpio.zst",
            "filesystem",
            4 * MIB - 12 * 1024,
            SizeChange::Changed {
                previous_bytes: 4 * MIB,
            },
        );
        assert_eq!(size_change_text(&shrank), "(-12.0 KiB)");
        let same = record("sdcard.img", "disk", MIB, SizeChange::Unchanged);
        assert_eq!(size_change_text(&same), "=");
        let added = record("kernel8.img", "disk", 3 * MIB, SizeChange::New);
        assert_eq!(size_change_text(&added), "new");
        let gone = record("old.img", "disk", 9 * MIB, SizeChange::Removed);
        assert_eq!(size_change_text(&gone), "removed (was 9.0 MiB)");
    }

    #[test]
    fn table_lines_align_kind_name_and_size() {
        let sizes = vec![
            record("bundle.tar.xz", "archive", 412 * MIB, SizeChange::Unchanged),
            record(
                "rootfs.erofs",
                "filesystem",
                182 * MIB + MIB / 2,
                SizeChange::Changed {
                    previous_bytes: 181 * MIB,
                },
            ),
            record("old.img", "disk", 9 * MIB, SizeChange::Removed),
        ];
        let lines = output_size_lines(&sizes);
        assert_eq!(lines.len(), 3);
        // Kind is padded to 10 columns, name to 13, size to 9 (the widest),
        // with two spaces between columns.
        assert_eq!(lines[0], "  archive     bundle.tar.xz  412.0 MiB  =");
        assert_eq!(
            lines[1],
            "  filesystem  rootfs.erofs   182.5 MiB  (+1.5 MiB)"
        );
        assert_eq!(
            lines[2],
            "  disk        old.img                -  removed (was 9.0 MiB)"
        );
    }

    #[test]
    fn no_sizes_print_no_rows() {
        assert!(output_size_lines(&[]).is_empty());
    }
}
