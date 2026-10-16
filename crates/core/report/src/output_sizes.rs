//! File sizes of the image outputs a run published, compared with the previous
//! successful run of the same build and variant.
//!
//! The comparison baseline is the size list of the last run that finished
//! cleanly, kept as `<build>.sizes.json` beside the reports. A failed or
//! cancelled run leaves it alone, so the next clean run is still compared with
//! the last good one. A reused image operation writes nothing, so its outputs
//! are carried over from the baseline: they are still on disk and still the
//! run's outputs.

use crate::assembly_outputs::KnownOutput;
use crate::model::{OutputSizeRecord, RunSummary, SizeChange};
use gaia_spec::ResolvedBuildSpec;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Which outputs a run's sizes cover.
pub(crate) struct SizeScope<'a> {
    /// The outputs the run published, the primary first.
    pub(crate) published: &'a [PathBuf],
    /// The outputs the previous summary listed, whether or not that run was
    /// clean. With `carry_over`, they are candidates too, so a reused image
    /// still reports its outputs before any baseline exists (the first run
    /// after the baseline was added).
    pub(crate) previous_outputs: &'a [PathBuf],
    /// The run reused an image operation, so the outputs that operation
    /// produced last time count as this run's outputs.
    pub(crate) carry_over: bool,
    /// The run finished cleanly and builds images, so a baseline output that
    /// is not among this run's outputs has been removed.
    pub(crate) removals: bool,
    /// Outputs the assembly spec names, with the kind the spec gives them.
    /// They count whenever they exist, reused or not.
    pub(crate) known: &'a [KnownOutput],
    /// The dirs output names are made relative to, the first that holds the
    /// output winning; otherwise the output's file name is its name.
    pub(crate) dirs: &'a [PathBuf],
}

/// The change of a size from `previous` to `current`, where either may be
/// absent: `None` when neither is (nothing to compare).
pub(crate) fn size_change(current: Option<u64>, previous: Option<u64>) -> Option<SizeChange> {
    match (current, previous) {
        (Some(_), None) => Some(SizeChange::New),
        (Some(bytes), Some(previous)) if bytes == previous => Some(SizeChange::Unchanged),
        (Some(_), Some(previous_bytes)) => Some(SizeChange::Changed { previous_bytes }),
        (None, Some(_)) => Some(SizeChange::Removed),
        (None, None) => None,
    }
}

/// The kind of an output, from its file name: `archive` for a tar or zip
/// (compressed or not), `disk` for an `.img`, `.raw` or `.iso` (compressed or
/// not), `filesystem` for an erofs, ext, squashfs, vfat or cpio image, and
/// `output` for anything else, such as a kernel or a verity hash. An `.img`
/// that is a transform output (a kernel image) is a `disk` by name.
pub(crate) fn output_kind(path: &Path) -> &'static str {
    let name = file_name(path).to_ascii_lowercase();
    if name.ends_with(".tgz") || name.ends_with(".zip") {
        return "archive";
    }
    let base = strip_compression(&name);
    if base.ends_with(".tar") {
        "archive"
    } else if [".img", ".raw", ".iso"]
        .iter()
        .any(|suffix| base.ends_with(suffix))
    {
        "disk"
    } else if FILESYSTEM_SUFFIXES
        .iter()
        .any(|suffix| base.ends_with(suffix))
    {
        "filesystem"
    } else {
        "output"
    }
}

const COMPRESSION_SUFFIXES: [&str; 6] = [".xz", ".zst", ".gz", ".bz2", ".lz4", ".lzma"];
const FILESYSTEM_SUFFIXES: [&str; 11] = [
    ".erofs",
    ".ext2",
    ".ext3",
    ".ext4",
    ".squashfs",
    ".vfat",
    ".fat",
    ".cpio",
    ".initramfs",
    ".cramfs",
    ".btrfs",
];

fn strip_compression(name: &str) -> &str {
    COMPRESSION_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
}

/// The size records of a run, sorted by kind then name. An output that is not
/// a file is left out. Each entry is compared with the baseline entry at the
/// same path; a baseline entry that is no longer an output is `removed` when
/// the run may say so.
pub(crate) fn output_size_records(
    scope: &SizeScope<'_>,
    baseline: &[OutputSizeRecord],
) -> Vec<OutputSizeRecord> {
    let mut candidates: Vec<PathBuf> = scope.known.iter().map(|known| known.path.clone()).collect();
    candidates.extend(scope.published.iter().cloned());
    if scope.carry_over {
        candidates.extend(scope.previous_outputs.iter().cloned());
        candidates.extend(baseline.iter().map(|record| PathBuf::from(&record.path)));
    }
    let mut seen = BTreeSet::<String>::new();
    let mut records = Vec::new();
    for path in candidates {
        let key = path.display().to_string();
        if seen.contains(&key) {
            continue;
        }
        let Some(bytes) = file_len(&path) else {
            continue;
        };
        seen.insert(key.clone());
        let previous = baseline
            .iter()
            .find(|record| record.path == key && record.change != SizeChange::Removed)
            .map(|record| record.bytes);
        let Some(change) = size_change(Some(bytes), previous) else {
            continue;
        };
        let kind = scope
            .known
            .iter()
            .find(|known| known.path == path)
            .map_or_else(|| output_kind(&path), |known| known.kind);
        records.push(OutputSizeRecord {
            kind: kind.to_string(),
            name: output_name(&path, scope.dirs),
            path: key,
            bytes,
            change,
        });
    }
    if scope.removals {
        for previous in baseline
            .iter()
            .filter(|record| record.change != SizeChange::Removed)
        {
            if seen.contains(&previous.path) {
                continue;
            }
            records.push(OutputSizeRecord {
                kind: previous.kind.clone(),
                name: previous.name.clone(),
                path: previous.path.clone(),
                bytes: previous.bytes,
                change: SizeChange::Removed,
            });
        }
    }
    records.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    records
}

/// The baseline file of `spec`: the sizes of its last clean run, beside its
/// reports and named for its build and variant.
pub(crate) fn baseline_path(spec: &ResolvedBuildSpec) -> PathBuf {
    crate::output::report_dir(spec).join(format!(
        "{}.sizes.json",
        crate::output::report_file_stem(spec)
    ))
}

/// The summary file of `spec`: the run's report, written on every run.
pub(crate) fn summary_path(spec: &ResolvedBuildSpec) -> PathBuf {
    crate::output::report_dir(spec).join(format!(
        "{}.summary.json",
        crate::output::report_file_stem(spec)
    ))
}

/// The `published_outputs` the summary file last written for `spec` lists, or
/// none when it is missing or cannot be read.
pub(crate) fn read_previous_outputs(summary_path: &Path) -> Vec<PathBuf> {
    let Some(summary) = fs::read(summary_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    summary
        .get("published_outputs")
        .and_then(serde_json::Value::as_array)
        .map(|outputs| {
            outputs
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// The baseline sizes, or none when there is no baseline or it cannot be read.
pub(crate) fn read_baseline(path: &Path) -> Vec<OutputSizeRecord> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the summary's sizes as the next baseline, when the run finished
/// cleanly and has outputs to record. Otherwise the previous baseline stays.
pub(crate) fn write_baseline(spec: &ResolvedBuildSpec, summary: &RunSummary) -> io::Result<()> {
    if summary.error_count != 0 || summary.cancelled {
        return Ok(());
    }
    let entries: Vec<OutputSizeRecord> = summary
        .sizes
        .iter()
        .filter(|record| record.change != SizeChange::Removed)
        .cloned()
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(&entries).map_err(io::Error::other)?;
    fs::write(baseline_path(spec), bytes)
}

fn file_len(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(metadata.len())
}

/// The name of an output: its path relative to the first of `dirs` that holds
/// it, which keeps names unique within an assembly (`initramfs.cpio`, or
/// `boot/kernel8.img`); the file name when no dir holds it.
fn output_name(path: &Path, dirs: &[PathBuf]) -> String {
    dirs.iter()
        .find_map(|dir| {
            path.strip_prefix(dir)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| relative.display().to_string())
        })
        .unwrap_or_else(|| file_name(path))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn size_change_covers_grow_shrink_equal_new_and_removed() {
        assert_eq!(
            size_change(Some(150), Some(100)),
            Some(SizeChange::Changed {
                previous_bytes: 100
            })
        );
        assert_eq!(
            size_change(Some(60), Some(100)),
            Some(SizeChange::Changed {
                previous_bytes: 100
            })
        );
        assert_eq!(
            size_change(Some(100), Some(100)),
            Some(SizeChange::Unchanged)
        );
        assert_eq!(size_change(Some(100), None), Some(SizeChange::New));
        assert_eq!(size_change(None, Some(100)), Some(SizeChange::Removed));
        assert_eq!(size_change(None, None), None);
    }

    #[test]
    fn delta_bytes_is_signed_and_absent_for_new_and_removed() {
        let record = |bytes, change| OutputSizeRecord {
            kind: "disk".into(),
            name: "a.img".into(),
            path: "/out/a.img".into(),
            bytes,
            change,
        };
        let grew = record(
            1_300,
            SizeChange::Changed {
                previous_bytes: 100,
            },
        );
        let shrank = record(
            88,
            SizeChange::Changed {
                previous_bytes: 100,
            },
        );
        assert_eq!(grew.delta_bytes(), Some(1_200));
        assert_eq!(shrank.delta_bytes(), Some(-12));
        assert_eq!(record(100, SizeChange::Unchanged).delta_bytes(), Some(0));
        assert_eq!(record(100, SizeChange::New).delta_bytes(), None);
        assert_eq!(record(100, SizeChange::Removed).delta_bytes(), None);
    }

    #[test]
    fn kinds_come_from_the_file_name() {
        assert_eq!(output_kind(Path::new("bundle.tar")), "archive");
        assert_eq!(output_kind(Path::new("bundle.tar.zst")), "archive");
        assert_eq!(output_kind(Path::new("slot.tgz")), "archive");
        assert_eq!(output_kind(Path::new("sdcard.img")), "disk");
        assert_eq!(output_kind(Path::new("sdcard.img.xz")), "disk");
        assert_eq!(output_kind(Path::new("disk.raw.zst")), "disk");
        assert_eq!(output_kind(Path::new("rootfs.erofs")), "filesystem");
        assert_eq!(output_kind(Path::new("boot.vfat")), "filesystem");
        assert_eq!(output_kind(Path::new("initramfs.cpio.zst")), "filesystem");
        assert_eq!(output_kind(Path::new("rootfs.verity")), "output");
    }

    #[test]
    fn records_compare_each_output_with_the_baseline() {
        let scratch = TempDir::new("gaia-report-sizes");
        let dir = scratch.join("out");
        fs::create_dir_all(&dir).expect("out dir");
        let grown = dir.join("rootfs.erofs");
        let same = dir.join("sdcard.img");
        let fresh = dir.join("initramfs.cpio.zst");
        fs::write(&grown, vec![0u8; 300]).expect("grown");
        fs::write(&same, vec![0u8; 50]).expect("same");
        fs::write(&fresh, vec![0u8; 7]).expect("fresh");
        let baseline = vec![
            baseline_record(&grown, 200),
            baseline_record(&same, 50),
            baseline_record(&dir.join("gone.img"), 9),
        ];
        let published = [grown.clone(), same.clone(), fresh.clone()];
        let records = output_size_records(
            &SizeScope {
                published: &published,
                previous_outputs: &[],
                known: &[],
                dirs: &[],
                carry_over: false,
                removals: true,
            },
            &baseline,
        );
        let by_name = |name: &str| {
            records
                .iter()
                .find(|record| record.name == name)
                .unwrap_or_else(|| panic!("no record for {name}: {records:?}"))
        };
        assert_eq!(
            by_name("rootfs.erofs").change,
            SizeChange::Changed {
                previous_bytes: 200
            }
        );
        assert_eq!(by_name("rootfs.erofs").kind, "filesystem");
        assert_eq!(by_name("sdcard.img").change, SizeChange::Unchanged);
        assert_eq!(by_name("initramfs.cpio.zst").change, SizeChange::New);
        let removed = by_name("gone.img");
        assert_eq!(removed.change, SizeChange::Removed);
        assert_eq!(removed.bytes, 9);
        // Sorted by kind then name: disk, filesystem, output.
        let order: Vec<&str> = records.iter().map(|record| record.name.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "gone.img",
                "sdcard.img",
                "initramfs.cpio.zst",
                "rootfs.erofs"
            ]
        );
    }

    #[test]
    fn reused_outputs_are_carried_over_from_the_baseline() {
        let scratch = TempDir::new("gaia-report-sizes");
        let dir = scratch.join("out");
        fs::create_dir_all(&dir).expect("out dir");
        let kept = dir.join("sdcard.img.xz");
        fs::write(&kept, vec![0u8; 40]).expect("kept");
        let baseline = vec![baseline_record(&kept, 40)];
        let records = output_size_records(
            &SizeScope {
                published: &[],
                previous_outputs: &[],
                known: &[],
                dirs: &[],
                carry_over: true,
                removals: true,
            },
            &baseline,
        );
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].change, SizeChange::Unchanged);
        assert_eq!(records[0].kind, "disk");
        // Without carry-over the same baseline entry is not an output of this
        // run, so it is removed.
        let without = output_size_records(
            &SizeScope {
                published: &[],
                previous_outputs: &[],
                known: &[],
                dirs: &[],
                carry_over: false,
                removals: true,
            },
            &baseline,
        );
        assert_eq!(without[0].change, SizeChange::Removed);
    }

    #[test]
    fn reused_output_without_a_baseline_comes_from_the_previous_summary() {
        let scratch = TempDir::new("gaia-report-sizes");
        let dir = scratch.join("out");
        fs::create_dir_all(&dir).expect("out dir");
        let kept = dir.join("sdcard.img");
        fs::write(&kept, vec![0u8; 40]).expect("kept");
        let previous = vec![kept.clone()];
        let records = output_size_records(
            &SizeScope {
                published: &[],
                previous_outputs: &previous,
                known: &[],
                dirs: &[],
                carry_over: true,
                removals: true,
            },
            &[],
        );
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].change, SizeChange::New);
        assert_eq!(records[0].bytes, 40);
        // Not reused, the previous summary's outputs are not this run's.
        let not_reused = output_size_records(
            &SizeScope {
                published: &[],
                previous_outputs: &previous,
                known: &[],
                dirs: &[],
                carry_over: false,
                removals: true,
            },
            &[],
        );
        assert!(not_reused.is_empty(), "{not_reused:?}");
    }

    #[test]
    fn removals_are_not_reported_when_the_run_may_not_say() {
        let scratch = TempDir::new("gaia-report-sizes");
        let gone = scratch.join("gone.img");
        let baseline = vec![baseline_record(&gone, 9)];
        let records = output_size_records(
            &SizeScope {
                published: &[],
                previous_outputs: &[],
                known: &[],
                dirs: &[],
                carry_over: false,
                removals: false,
            },
            &baseline,
        );
        assert!(records.is_empty(), "{records:?}");
    }

    #[test]
    fn baseline_round_trips_and_ignores_removed_entries() {
        let scratch = TempDir::new("gaia-report-sizes");
        let path = scratch.join("build.sizes.json");
        let entries = [
            baseline_record(&scratch.join("a.img"), 10),
            OutputSizeRecord {
                change: SizeChange::Removed,
                ..baseline_record(&scratch.join("b.img"), 20)
            },
        ];
        let kept: Vec<OutputSizeRecord> = entries
            .iter()
            .filter(|record| record.change != SizeChange::Removed)
            .cloned()
            .collect();
        fs::write(&path, serde_json::to_vec(&kept).expect("json")).expect("write");
        assert_eq!(read_baseline(&path), kept);
        assert!(read_baseline(&scratch.join("missing.json")).is_empty());
    }

    fn baseline_record(path: &Path, bytes: u64) -> OutputSizeRecord {
        OutputSizeRecord {
            kind: output_kind(path).to_string(),
            name: file_name(path),
            path: path.display().to_string(),
            bytes,
            change: SizeChange::New,
        }
    }
}
