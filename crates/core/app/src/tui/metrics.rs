//! Machine metrics for the dashboard, sampled once a second from `/proc`:
//! CPU use (from the deltas of `/proc/stat`), memory and tmpfs (`/proc/meminfo`;
//! `Shmem` is what `/dev/shm` and other tmpfs mounts hold), load average and
//! disk throughput (`/proc/diskstats`, whole disks only). Each is a ring
//! buffer of the last few minutes.
//!
//! The parsers take the file text, and the sampler reads from a directory
//! that is `/proc` in production, so tests run it on a fixture tree.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the metrics are sampled.
pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Samples kept per series: five minutes at one a second.
pub(crate) const HISTORY: usize = 300;
/// A disk sector, as `/proc/diskstats` counts them.
const SECTOR_BYTES: f64 = 512.0;

/// The last [`HISTORY`] samples of one metric, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Series {
    values: VecDeque<f64>,
}

impl Series {
    pub(crate) fn push(&mut self, value: f64) {
        if self.values.len() == HISTORY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub(crate) fn last(&self) -> Option<f64> {
        self.values.back().copied()
    }

    pub(crate) fn max(&self) -> f64 {
        self.values.iter().copied().fold(0.0, f64::max)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    /// The newest `width` samples, scaled to whole numbers for a sparkline:
    /// `value * scale`, with `max` (or the series' own maximum when `max` is
    /// zero) mapped to `scale`.
    pub(crate) fn sparkline(&self, width: usize, scale: f64, max: f64) -> Vec<u64> {
        let max = if max > 0.0 { max } else { self.max() };
        let skip = self.values.len().saturating_sub(width);
        self.values
            .iter()
            .skip(skip)
            .map(|value| {
                if max <= 0.0 {
                    0
                } else {
                    (value / max * scale).round().max(0.0) as u64
                }
            })
            .collect()
    }
}

/// Busy and total jiffies of the whole CPU, as `/proc/stat` counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CpuTimes {
    pub(crate) busy: u64,
    pub(crate) total: u64,
}

/// The aggregate `cpu` line of `/proc/stat`.
pub(crate) fn parse_cpu_times(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields = line
        .split_whitespace()
        .skip(1)
        .map(|field| field.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    // idle and iowait are the time nothing ran.
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// The share of CPU time spent busy between two readings, in percent.
pub(crate) fn cpu_percent(previous: CpuTimes, now: CpuTimes) -> Option<f64> {
    let total = now.total.checked_sub(previous.total)?;
    let busy = now.busy.checked_sub(previous.busy)?;
    (total > 0).then(|| busy as f64 * 100.0 / total as f64)
}

/// The memory figures of `/proc/meminfo`, in KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemInfo {
    pub(crate) total_kib: u64,
    pub(crate) available_kib: u64,
    pub(crate) shmem_kib: u64,
}

pub(crate) fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<u64>()
                .ok()
        })
    };
    Some(MemInfo {
        total_kib: field("MemTotal:")?,
        available_kib: field("MemAvailable:")?,
        shmem_kib: field("Shmem:").unwrap_or(0),
    })
}

/// The one-minute load average of `/proc/loadavg`.
pub(crate) fn parse_loadavg(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// Sectors read and written by all whole disks, from `/proc/diskstats`.
pub(crate) fn parse_diskstats(text: &str) -> (u64, u64) {
    text.lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let name = *fields.get(2)?;
            if !is_whole_disk(name) {
                return None;
            }
            let read = fields.get(5)?.parse::<u64>().ok()?;
            let written = fields.get(9)?.parse::<u64>().ok()?;
            Some((read, written))
        })
        .fold((0, 0), |(read, written), (r, w)| (read + r, written + w))
}

/// Whether a `/proc/diskstats` device is a whole disk rather than a
/// partition, a loop, RAM or device-mapper device.
fn is_whole_disk(name: &str) -> bool {
    const SKIPPED: [&str; 7] = ["loop", "ram", "zram", "dm-", "md", "sr", "fd"];
    if SKIPPED.iter().any(|prefix| name.starts_with(prefix)) {
        return false;
    }
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        // nvme0n1 is a disk, nvme0n1p1 a partition.
        return !has_partition_suffix(name);
    }
    // sda is a disk, sda1 a partition.
    name.chars().last().is_some_and(|c| c.is_ascii_alphabetic())
}

fn has_partition_suffix(name: &str) -> bool {
    match name.rfind('p') {
        Some(position) if position > 0 => {
            let (head, tail) = (&name[..position], &name[position + 1..]);
            !tail.is_empty()
                && tail.chars().all(|c| c.is_ascii_digit())
                && head.chars().last().is_some_and(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// The sampled metrics of the machine.
pub(crate) struct Metrics {
    root: PathBuf,
    last_sample: Option<Instant>,
    previous_cpu: Option<CpuTimes>,
    /// Sectors read and written at the previous sample, and when.
    previous_disk: Option<(u64, u64, Instant)>,
    /// CPU busy, percent.
    pub(crate) cpu: Series,
    /// Memory used, percent of the total.
    pub(crate) memory: Series,
    pub(crate) memory_used_kib: u64,
    pub(crate) memory_total_kib: u64,
    /// tmpfs (`Shmem`), MiB.
    pub(crate) shm: Series,
    /// One-minute load average.
    pub(crate) load: Series,
    /// Disk read and write throughput, bytes per second.
    pub(crate) disk_read: Series,
    pub(crate) disk_write: Series,
}

impl Metrics {
    /// Metrics read from `root`, normally `/proc`.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            last_sample: None,
            previous_cpu: None,
            previous_disk: None,
            cpu: Series::default(),
            memory: Series::default(),
            memory_used_kib: 0,
            memory_total_kib: 0,
            shm: Series::default(),
            load: Series::default(),
            disk_read: Series::default(),
            disk_write: Series::default(),
        }
    }

    /// Metrics of this machine.
    pub(crate) fn system() -> Self {
        Self::new("/proc")
    }

    /// Samples when a sample is due. Returns whether it did.
    pub(crate) fn sample_if_due(&mut self, now: Instant) -> bool {
        let due = self
            .last_sample
            .is_none_or(|at| now.duration_since(at) >= SAMPLE_INTERVAL);
        if due {
            self.sample(now);
        }
        due
    }

    /// Takes one sample now. A file that cannot be read leaves its series as
    /// it was.
    pub(crate) fn sample(&mut self, now: Instant) {
        self.last_sample = Some(now);
        if let Some(current) = read(&self.root, "stat").and_then(|text| parse_cpu_times(&text)) {
            if let Some(percent) = self
                .previous_cpu
                .and_then(|previous| cpu_percent(previous, current))
            {
                self.cpu.push(percent);
            }
            self.previous_cpu = Some(current);
        }
        if let Some(memory) = read(&self.root, "meminfo").and_then(|text| parse_meminfo(&text)) {
            let used = memory.total_kib.saturating_sub(memory.available_kib);
            self.memory_used_kib = used;
            self.memory_total_kib = memory.total_kib;
            if memory.total_kib > 0 {
                self.memory
                    .push(used as f64 * 100.0 / memory.total_kib as f64);
            }
            self.shm.push(memory.shmem_kib as f64 / 1_024.0);
        }
        if let Some(load) = read(&self.root, "loadavg").and_then(|text| parse_loadavg(&text)) {
            self.load.push(load);
        }
        if let Some(text) = read(&self.root, "diskstats") {
            let (read_sectors, written_sectors) = parse_diskstats(&text);
            if let Some((previous_read, previous_written, at)) = self.previous_disk {
                let seconds = now.duration_since(at).as_secs_f64();
                if seconds > 0.0 {
                    let rate = |now: u64, before: u64| {
                        now.saturating_sub(before) as f64 * SECTOR_BYTES / seconds
                    };
                    self.disk_read.push(rate(read_sectors, previous_read));
                    self.disk_write
                        .push(rate(written_sectors, previous_written));
                }
            }
            self.previous_disk = Some((read_sectors, written_sectors, now));
        }
    }
}

fn read(root: &Path, name: &str) -> Option<String> {
    fs::read_to_string(root.join(name)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    const STAT_1: &str = "cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 50 0 50 400 0 0 0 0 0 0\n";
    const STAT_2: &str = "cpu  300 0 200 1000 0 0 0 0 0 0\ncpu0 1 0 1 1 0 0 0 0 0 0\n";
    const MEMINFO: &str = "MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:   12000000 kB\nShmem:            1048576 kB\n";
    const DISKSTATS: &str = "   8       0 sda 10 0 2048 0 5 0 0 0 0 0 0\n   8       1 sda1 10 0 1024 0 5 0 512 0 0 0 0\n 259       0 nvme0n1 1 0 100 0 1 0 200 0 0 0 0\n 259       1 nvme0n1p1 1 0 50 0 1 0 100 0 0 0 0\n   7       0 loop0 1 0 9999 0 0 0 0 0 0 0 0\n";

    #[test]
    fn cpu_percent_is_busy_share_of_the_delta() {
        let first = parse_cpu_times(STAT_1).expect("first");
        let second = parse_cpu_times(STAT_2).expect("second");
        // Between the readings: 500 jiffies passed, 300 of them busy.
        assert_eq!(
            first,
            CpuTimes {
                busy: 200,
                total: 1_000
            }
        );
        assert_eq!(
            second,
            CpuTimes {
                busy: 500,
                total: 1_500
            }
        );
        assert_eq!(cpu_percent(first, second), Some(60.0));
        assert_eq!(cpu_percent(second, second), None, "no time passed");
        assert_eq!(parse_cpu_times("intr 1 2 3"), None);
    }

    #[test]
    fn meminfo_reads_totals_and_shmem() {
        let memory = parse_meminfo(MEMINFO).expect("meminfo");
        assert_eq!(memory.total_kib, 16_000_000);
        assert_eq!(memory.available_kib, 12_000_000);
        assert_eq!(memory.shmem_kib, 1_048_576);
        assert_eq!(parse_meminfo("MemFree: 1 kB"), None);
    }

    #[test]
    fn loadavg_takes_the_first_field() {
        assert_eq!(parse_loadavg("0.52 0.58 0.59 2/1234 5678\n"), Some(0.52));
        assert_eq!(parse_loadavg(""), None);
    }

    #[test]
    fn diskstats_counts_whole_disks_only() {
        // sda: 2048 sectors read, 0 written; nvme0n1: 100 read, 200 written.
        // Partitions, loop devices and their sectors are left out.
        assert_eq!(parse_diskstats(DISKSTATS), (2_148, 200));
        assert!(is_whole_disk("sda"));
        assert!(!is_whole_disk("sda1"));
        assert!(is_whole_disk("nvme0n1"));
        assert!(!is_whole_disk("nvme0n1p1"));
        assert!(is_whole_disk("mmcblk0"));
        assert!(!is_whole_disk("mmcblk0p2"));
        assert!(!is_whole_disk("dm-0"));
    }

    #[test]
    fn series_keeps_the_newest_samples_and_scales_for_sparklines() {
        let mut series = Series::default();
        for value in 0..(HISTORY + 5) {
            series.push(value as f64);
        }
        assert_eq!(series.len(), HISTORY);
        assert_eq!(series.last(), Some((HISTORY + 4) as f64));
        assert_eq!(series.sparkline(3, 10.0, 0.0).len(), 3);
        let scaled = Series {
            values: VecDeque::from(vec![0.0, 50.0, 100.0]),
        }
        .sparkline(3, 10.0, 0.0);
        assert_eq!(scaled, vec![0, 5, 10]);
        // A fixed maximum keeps two machines' charts comparable.
        let fixed = Series {
            values: VecDeque::from(vec![25.0]),
        }
        .sparkline(1, 100.0, 100.0);
        assert_eq!(fixed, vec![25]);
    }

    #[test]
    fn sampler_reads_a_proc_tree_and_rates_disks() {
        let scratch = TempDir::new("gaia-metrics-proc");
        let root = scratch.path();
        fs::write(root.join("stat"), STAT_1).expect("stat");
        fs::write(root.join("meminfo"), MEMINFO).expect("meminfo");
        fs::write(root.join("loadavg"), "1.50 1 1 1/1 1\n").expect("loadavg");
        fs::write(root.join("diskstats"), DISKSTATS).expect("diskstats");

        let mut metrics = Metrics::new(root);
        let start = Instant::now();
        assert!(metrics.sample_if_due(start));
        // A second sample within the interval is not due.
        assert!(!metrics.sample_if_due(start + Duration::from_millis(200)));
        assert_eq!(metrics.cpu.len(), 0, "a rate needs two readings");
        assert_eq!(metrics.load.last(), Some(1.5));
        assert_eq!(metrics.memory_total_kib, 16_000_000);
        assert!((metrics.memory.last().expect("memory") - 25.0).abs() < 1e-9);
        assert_eq!(metrics.shm.last(), Some(1_024.0));

        fs::write(root.join("stat"), STAT_2).expect("stat");
        fs::write(root.join("diskstats"), DISKSTATS.replace("2048", "4096")).expect("diskstats");
        assert!(metrics.sample_if_due(start + Duration::from_secs(2)));
        assert_eq!(metrics.cpu.last(), Some(60.0));
        // 2048 more sectors (1 MiB) were read in 2 s: 512 KiB/s.
        assert_eq!(metrics.disk_read.last(), Some(524_288.0));
    }

    #[test]
    fn a_missing_proc_tree_leaves_the_series_empty() {
        let scratch = TempDir::new("gaia-metrics-missing");
        let mut metrics = Metrics::new(scratch.path().join("absent"));
        metrics.sample(Instant::now());
        assert_eq!(
            metrics.cpu.len() + metrics.load.len() + metrics.shm.len(),
            0
        );
    }
}
