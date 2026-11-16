//! Where a step's scratch directory is: on the store's disk, or in RAM.
//!
//! A Buildroot build writes tens of gigabytes of small files, which takes
//! most of its time on the store's disk. Such a step may work in RAM instead,
//! under `/dev/shm` (never `/tmp`), and its finished outputs are copied to the
//! store when it commits. The choice is made here:
//!
//! - `GAIA_WORK_DIR=disk` always uses the disk; `ram` uses RAM whenever the
//!   cap allows; `auto` (the default) uses RAM when the step's expected size
//!   fits in the memory left after a reserve, and in `/dev/shm`.
//! - `GAIA_RAM_MAX` (default 60G) caps the RAM work directories of one
//!   process together. A RAM work directory counts from its creation until
//!   the step ends.
//!
//! A RAM work directory is `/dev/shm/gaia-<store id>/<scratch name>`, where
//! the store id is a short digest of the store's root path, so two stores on
//! one machine do not share a directory.

use crate::{Digest, Error, Result};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// Memory the automatic choice leaves free, whatever a step expects.
pub const RESERVE_BYTES: u64 = 16 << 30;

/// The default cap on the RAM work directories of one process.
pub const DEFAULT_RAM_MAX: u64 = 60 << 30;

/// Where RAM work directories are made.
pub const SHM: &str = "/dev/shm";

/// The process-wide budget of RAM work directories.
pub(crate) static BUDGET: RamBudget = RamBudget::new();

/// What a step may work in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Disk,
    Ram,
    Auto,
}

impl Mode {
    /// Reads `disk`, `ram` or `auto`; an empty word is `auto`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "disk" => Some(Self::Disk),
            "ram" => Some(Self::Ram),
            "auto" | "" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// The settings a process reads from its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    pub ram_max: u64,
}

impl Settings {
    /// Reads `GAIA_WORK_DIR` and `GAIA_RAM_MAX`. A value that does not parse
    /// is an error, since a silent default would hide what the user asked.
    pub fn from_env() -> Result<Self> {
        let mode = match std::env::var("GAIA_WORK_DIR") {
            Ok(text) => Mode::parse(&text).ok_or_else(|| {
                Error::work(format!("GAIA_WORK_DIR={text}: expected disk, ram or auto"))
            })?,
            Err(_) => Mode::Auto,
        };
        let ram_max = match std::env::var("GAIA_RAM_MAX") {
            Ok(text) => parse_size(&text).ok_or_else(|| {
                Error::work(format!(
                    "GAIA_RAM_MAX={text}: expected a size such as 60G or 512M"
                ))
            })?,
            Err(_) => DEFAULT_RAM_MAX,
        };
        Ok(Self { mode, ram_max })
    }
}

/// A size such as `60G`, `512m`, `2GiB` or plain bytes. Units are powers of
/// 1024.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let number: u64 = text[..digits].parse().ok()?;
    let shift = match text[digits..].trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => return None,
    };
    number.checked_mul(1 << shift)
}

/// What the machine has free right now: the memory the kernel expects to
/// give without swapping, and the room in `/dev/shm`. A value that cannot be
/// read is `None`, and the automatic choice then uses the disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Room {
    pub mem_available: Option<u64>,
    pub shm_free: Option<u64>,
}

impl Room {
    /// Reads `/proc/meminfo` and the free space of `shm`.
    pub fn read(shm: &Path) -> Self {
        let mem_available = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| parse_mem_available(&text));
        let shm_free = rustix::fs::statvfs(shm)
            .ok()
            .map(|stat| stat.f_bavail.saturating_mul(stat.f_frsize));
        Self {
            mem_available,
            shm_free,
        }
    }
}

/// The `MemAvailable` line of `/proc/meminfo`, in bytes.
pub fn parse_mem_available(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        let kib: u64 = rest.trim().strip_suffix("kB")?.trim().parse().ok()?;
        kib.checked_mul(1024)
    })
}

/// Whether the automatic choice takes RAM for a step expecting `expected`
/// bytes: the memory left after the reserve, and `/dev/shm`, must each hold
/// it.
pub(crate) fn auto_fits(room: Room, expected: u64) -> bool {
    match (room.mem_available, room.shm_free) {
        (Some(mem), Some(shm)) => mem.saturating_sub(RESERVE_BYTES) >= expected && shm >= expected,
        _ => false,
    }
}

/// The directory RAM work directories of the store at `root` are made in,
/// under `shm`.
pub fn ram_root(root: &Path, shm: &Path) -> PathBuf {
    let id = Digest::of_bytes(root.as_os_str().as_bytes()).hex();
    shm.join(format!("gaia-{}", &id[..16]))
}

/// The RAM work directories that are in use by this process, in bytes, and
/// the cap on them. A claim is made before a RAM work directory is created
/// and returned when the directory is gone.
#[derive(Debug)]
pub(crate) struct RamBudget {
    used: Mutex<u64>,
}

impl RamBudget {
    pub(crate) const fn new() -> Self {
        Self {
            used: Mutex::new(0),
        }
    }

    /// Claims `bytes` if they fit under `cap` together with what is claimed.
    pub(crate) fn claim(&'static self, bytes: u64, cap: u64) -> Option<RamClaim> {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        let total = used.checked_add(bytes)?;
        if total > cap {
            return None;
        }
        *used = total;
        Some(RamClaim {
            budget: self,
            bytes,
        })
    }

    /// Bytes currently claimed.
    pub(crate) fn in_use(&self) -> u64 {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A claim on a [`RamBudget`], returned when dropped.
#[derive(Debug)]
pub(crate) struct RamClaim {
    budget: &'static RamBudget,
    bytes: u64,
}

impl Drop for RamClaim {
    fn drop(&mut self) {
        let mut used = self
            .budget
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn room(mem_gib: u64, shm_gib: u64) -> Room {
        Room {
            mem_available: Some(mem_gib * GIB),
            shm_free: Some(shm_gib * GIB),
        }
    }

    #[test]
    fn auto_takes_ram_when_memory_and_shm_hold_the_step() {
        // 40 GiB free, 16 reserved: 24 are left, which holds a 24 GiB step.
        assert!(auto_fits(room(40, 64), 24 * GIB));
    }

    #[test]
    fn auto_keeps_the_reserve_free() {
        assert!(!auto_fits(room(39, 64), 24 * GIB));
        assert!(auto_fits(room(40, 64), 24 * GIB));
    }

    #[test]
    fn auto_needs_room_in_shm_too() {
        assert!(!auto_fits(room(100, 8), 24 * GIB));
    }

    #[test]
    fn auto_uses_the_disk_when_a_reading_is_missing() {
        assert!(!auto_fits(Room::default(), 1));
        let no_shm = Room {
            mem_available: Some(100 * GIB),
            shm_free: None,
        };
        assert!(!auto_fits(no_shm, 1));
    }

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse("disk"), Some(Mode::Disk));
        assert_eq!(Mode::parse(" ram "), Some(Mode::Ram));
        assert_eq!(Mode::parse("auto"), Some(Mode::Auto));
        assert_eq!(Mode::parse(""), Some(Mode::Auto));
        assert_eq!(Mode::parse("nvme"), None);
    }

    #[test]
    fn sizes_parse_with_units() {
        assert_eq!(parse_size("60G"), Some(60 * GIB));
        assert_eq!(parse_size("512m"), Some(512 << 20));
        assert_eq!(parse_size("2GiB"), Some(2 * GIB));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("G"), None);
        assert_eq!(parse_size("60X"), None);
    }

    #[test]
    fn memavailable_is_read_in_bytes() {
        let meminfo = "MemTotal:       65536000 kB\nMemAvailable:   20971520 kB\n";
        assert_eq!(parse_mem_available(meminfo), Some(20 * GIB));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn the_cap_counts_claims_until_they_are_dropped() {
        let budget: &'static RamBudget = Box::leak(Box::new(RamBudget::new()));
        let first = budget.claim(4, 10).expect("4 fits in 10");
        let second = budget.claim(4, 10).expect("8 fits in 10");
        assert!(budget.claim(4, 10).is_none(), "12 must not fit in 10");
        assert_eq!(budget.in_use(), 8);
        drop(first);
        assert_eq!(budget.in_use(), 4);
        let third = budget.claim(4, 10).expect("after a release it fits");
        drop((second, third));
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn concurrent_claims_never_pass_the_cap() {
        let budget: &'static RamBudget = Box::leak(Box::new(RamBudget::new()));
        let cap = 100;
        let peak = std::sync::atomic::AtomicU64::new(0);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        if let Some(claim) = budget.claim(10, cap) {
                            peak.fetch_max(budget.in_use(), std::sync::atomic::Ordering::SeqCst);
                            drop(claim);
                        }
                    }
                });
            }
        });
        assert!(peak.into_inner() <= cap);
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn ram_roots_differ_per_store() {
        let shm = Path::new("/dev/shm");
        let one = ram_root(Path::new("/a/store"), shm);
        let two = ram_root(Path::new("/b/store"), shm);
        assert_ne!(one, two);
        assert_eq!(one.parent(), Some(shm));
        assert_eq!(ram_root(Path::new("/a/store"), shm), one);
    }
}
