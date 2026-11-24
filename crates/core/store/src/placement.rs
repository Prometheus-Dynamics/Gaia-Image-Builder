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
//! Work inside Gaia's own process that is about to use much memory (an
//! in-process encoder's buffers) holds it with [`hold_memory`] while it
//! runs; the automatic choice counts held memory as taken.
//!
//! A RAM work directory is `/dev/shm/gaia-<store id>/<scratch name>`, where
//! the store id is a short digest of the store's root path, so two stores on
//! one machine do not share a directory.
//!
//! Nothing is left there after a run: a work directory goes when its step
//! ends, and the `gaia-<store id>` directory with the last one. A process
//! that is killed cannot do that, so the first RAM work directory of each
//! process removes what killed ones left (see [`sweep`]).

use crate::{Digest, Error, Result};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
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
/// give without swapping, the room in `/dev/shm`, and what this process's
/// disk quota there still allows. A free value that cannot be read is
/// `None`, and the automatic choice then uses the disk. `shm_quota` is
/// `None` when no quota applies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Room {
    pub mem_available: Option<u64>,
    pub shm_free: Option<u64>,
    pub shm_quota: Option<u64>,
}

impl Room {
    /// Reads `/proc/meminfo` (less the memory held with [`hold_memory`]),
    /// the free space of `shm` and the quota on it
    /// (see [`quota_headroom`]). A quota that is on and cannot be read is an
    /// error: the free space alone would put a step where the quota refuses
    /// its writes. [`crate::Store::scratch_for`] then uses the disk.
    pub fn read(shm: &Path) -> Result<Self> {
        let mem_available = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| parse_mem_available(&text))
            .map(|available| available.saturating_sub(held_memory()));
        let shm_free = rustix::fs::statvfs(shm)
            .ok()
            .map(|stat| stat.f_bavail.saturating_mul(stat.f_frsize));
        let shm_quota = quota_headroom(shm)?;
        Ok(Self {
            mem_available,
            shm_free,
            shm_quota,
        })
    }

    /// The bytes `/dev/shm` takes now: its free space, or the quota's
    /// headroom when that is less.
    fn shm_room(&self) -> Option<u64> {
        let free = self.shm_free?;
        Some(self.shm_quota.map_or(free, |quota| quota.min(free)))
    }
}

/// Says once per process why the automatic choice put work on the disk:
/// what it could not read in `/dev/shm`.
pub(crate) fn note_disk(error: &Error) {
    static NOTED: std::sync::Once = std::sync::Once::new();
    NOTED.call_once(|| {
        eprintln!("gaia: work directories are on the disk: {error}");
    });
}

/// Which quotas a mount has on, from its options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuotaKinds {
    pub user: bool,
    pub group: bool,
    pub project: bool,
}

/// The mount `path` is on, from `/proc/self/mountinfo` text: its mount point
/// and the quotas its options turn on. `path` must be absolute and
/// canonical. The deepest mount point that holds `path` is its mount.
pub fn mount_quotas(mountinfo: &str, path: &Path) -> Option<(PathBuf, QuotaKinds)> {
    let mut found: Option<(PathBuf, QuotaKinds)> = None;
    for line in mountinfo.lines() {
        let Some((head, tail)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<&str> = head.split(' ').collect();
        let (Some(point), Some(options)) = (fields.get(4), fields.get(5)) else {
            continue;
        };
        let point = PathBuf::from(unescape_mount(point));
        if !path.starts_with(&point) {
            continue;
        }
        if found
            .as_ref()
            .is_some_and(|(deepest, _)| deepest.components().count() > point.components().count())
        {
            continue;
        }
        let super_options = tail.split(' ').nth(2).unwrap_or("");
        let mut kinds = QuotaKinds::default();
        for option in options.split(',').chain(super_options.split(',')) {
            let name = option.split('=').next().unwrap_or("");
            match name {
                "usrquota" | "usrjquota" | "quota" | "uquota" | "uqnoenforce" | "qnoenforce" => {
                    kinds.user = true;
                }
                "grpquota" | "grpjquota" | "gquota" | "gqnoenforce" => kinds.group = true,
                "prjquota" | "pquota" | "pqnoenforce" => kinds.project = true,
                _ => {}
            }
        }
        found = Some((point, kinds));
    }
    found
}

/// A mount point as mountinfo writes it, with `\040` and the like read back.
fn unescape_mount(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\'
            && at + 3 < bytes.len()
            && bytes[at + 1..at + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[at + 1] - b'0') * 64 + (bytes[at + 2] - b'0') * 8 + (bytes[at + 3] - b'0');
            out.push(value);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the quota report of `quota -v -w -p --show-mntpoint` allows still at
/// `mount`, in bytes: for each id it reports, the lower of its soft and hard
/// block limits that is set, less the blocks used (1 KiB blocks), and the
/// least of those. `None` when no id has a limit there. A report with no line
/// for `mount` is an error: `-v` lists every quota-enabled mount.
pub fn quota_report_headroom(report: &str, mount: &Path) -> Result<Option<u64>> {
    let mount_text = mount.display().to_string();
    let mut headroom: Option<u64> = None;
    let mut lines = 0;
    for line in report.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) != Some(&mount_text.as_str()) {
            continue;
        }
        lines += 1;
        let number = |at: usize, what: &str| -> Result<u64> {
            fields
                .get(at)
                .map(|text| text.trim_end_matches('*'))
                .and_then(|text| text.parse::<u64>().ok())
                .ok_or_else(|| {
                    Error::work(format!(
                        "the quota report's {what} for {mount_text} does not read: {line}"
                    ))
                })
        };
        let used = number(2, "blocks used")?;
        let soft = number(3, "soft block limit")?;
        let hard = number(4, "hard block limit")?;
        let limit = match (soft, hard) {
            (0, 0) => continue,
            (0, limit) | (limit, 0) => limit,
            (soft, hard) => soft.min(hard),
        };
        let left = limit.saturating_sub(used).saturating_mul(1024);
        headroom = Some(headroom.map_or(left, |least| least.min(left)));
    }
    if lines == 0 {
        return Err(Error::work(format!(
            "the quota report names no line for {mount_text}:\n{report}"
        )));
    }
    Ok(headroom)
}

/// What the disk quotas on the mount `shm` is on still allow this process
/// to write there, in bytes. `None` when the mount has no quota on (its
/// options in `/proc/self/mountinfo` turn none on), or no limit is set for
/// this user and its groups. A quota that is on is read with `quota`
/// (quota-tools); a tool that cannot be run, or fails, or a project quota,
/// which this does not read, is an error that names it.
pub fn quota_headroom(shm: &Path) -> Result<Option<u64>> {
    let path = fs::canonicalize(shm).map_err(|e| Error::work(format!("{}: {e}", shm.display())))?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|e| Error::work(format!("/proc/self/mountinfo: {e}")))?;
    quota_headroom_with(&path, &mountinfo, |flag, mount| {
        std::process::Command::new("quota")
            .args(["-v", "-w", "-p", "--show-mntpoint", flag, "-f"])
            .arg(mount)
            .env("LC_ALL", "C")
            .output()
    })
}

/// [`quota_headroom`] with the mount table and the way `quota` is run given,
/// so each answer can be tested. `run` takes `-u` or `-g` and the mount point.
pub(crate) fn quota_headroom_with(
    path: &Path,
    mountinfo: &str,
    run: impl Fn(&str, &Path) -> std::io::Result<std::process::Output>,
) -> Result<Option<u64>> {
    let (mount, kinds) = mount_quotas(mountinfo, path).ok_or_else(|| {
        Error::work(format!(
            "/proc/self/mountinfo has no mount that holds {}",
            path.display()
        ))
    })?;
    if kinds.project {
        return Err(Error::work(format!(
            "{} has project quotas on, which RAM placement does not read; \
             set GAIA_WORK_DIR=disk or ram to choose for it",
            mount.display()
        )));
    }
    let mut headroom: Option<u64> = None;
    for (on, flag) in [(kinds.user, "-u"), (kinds.group, "-g")] {
        if !on {
            continue;
        }
        let output = run(flag, &mount).map_err(|e| {
            Error::work(format!(
                "{} has quotas on ({flag}), and `quota` could not be run to read them: {e}",
                mount.display()
            ))
        })?;
        if !output.status.success() {
            return Err(Error::work(format!(
                "`quota {flag} -f {}` failed ({}): {}",
                mount.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let report = String::from_utf8_lossy(&output.stdout);
        if let Some(left) = quota_report_headroom(&report, &mount)? {
            headroom = Some(headroom.map_or(left, |least| least.min(left)));
        }
    }
    Ok(headroom)
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
/// bytes: the memory left after the reserve, and `/dev/shm` within its
/// quota, must each hold it.
pub(crate) fn auto_fits(room: Room, expected: u64) -> bool {
    match (room.mem_available, room.shm_room()) {
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

/// Removes what killed runs of this user left in `shm`: in each RAM root
/// (`gaia-` and 16 hex digits, a directory this user owns), the work
/// directories of processes that are gone (named by their pid, see
/// `fsutil::unique_name`), then the root itself if it is empty. Anything
/// else there is not looked at. Returns how many directories it removed.
pub(crate) fn sweep(shm: &Path) -> usize {
    let Ok(me) = fs::metadata("/proc/self").map(|m| m.uid()) else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(shm) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_prefix("gaia-")) else {
            continue;
        };
        if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let root = entry.path();
        let ours = fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir() && m.uid() == me);
        if !ours {
            continue;
        }
        for work in fs::read_dir(&root).into_iter().flatten().flatten() {
            let name = work.file_name().to_string_lossy().into_owned();
            if !crate::fsutil::name_pid(&name).is_some_and(crate::fsutil::process_alive)
                && crate::fsutil::remove_tree(&work.path()).is_ok()
            {
                removed += 1;
            }
        }
        // Only when empty: a live process's work directory keeps it.
        if fs::remove_dir(&root).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// [`sweep`], the first time this process asks for `shm`.
pub(crate) fn sweep_once(shm: &Path) {
    static SWEPT: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    let first = SWEPT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(shm.to_path_buf());
    if first {
        sweep(shm);
    }
}

/// Memory that work inside this process has said it is using or about to
/// use, in bytes.
static HELD: Mutex<u64> = Mutex::new(0);

/// Holds `bytes` of memory for in-process work until the hold is dropped:
/// [`Room::read`] counts them as taken, so a step placed in RAM meanwhile
/// does not count on them. Memory the work has already touched is gone
/// from `MemAvailable` too and is then counted twice, which errs towards
/// the disk.
pub fn hold_memory(bytes: u64) -> MemoryHold {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    *held = held.saturating_add(bytes);
    MemoryHold { bytes }
}

/// The memory held with [`hold_memory`] now, in bytes.
pub fn held_memory() -> u64 {
    *HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A hold from [`hold_memory`], returned when dropped.
#[derive(Debug)]
#[must_use = "the memory is held only while this lives"]
pub struct MemoryHold {
    bytes: u64,
}

impl Drop for MemoryHold {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        *held = held.saturating_sub(self.bytes);
    }
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

    /// Held memory adds up while its holds live, is returned when they are
    /// dropped, and is taken off the memory the automatic choice reads.
    #[test]
    fn held_memory_is_counted_until_dropped() {
        let before = held_memory();
        let one = hold_memory(3 * GIB);
        let two = hold_memory(GIB);
        assert_eq!(held_memory() - before, 4 * GIB);
        drop(one);
        assert_eq!(held_memory() - before, GIB);
        let all = hold_memory(u64::MAX / 2);
        let room = Room::read(Path::new("/proc")).expect("room");
        assert_eq!(room.mem_available, Some(0), "held memory is not counted");
        drop((two, all));
        assert_eq!(held_memory(), before);
    }

    fn room(mem_gib: u64, shm_gib: u64) -> Room {
        Room {
            mem_available: Some(mem_gib * GIB),
            shm_free: Some(shm_gib * GIB),
            shm_quota: None,
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
            shm_quota: None,
        };
        assert!(!auto_fits(no_shm, 1));
    }

    #[test]
    fn auto_needs_room_within_the_shm_quota() {
        // 52 GiB free in /dev/shm, but the quota leaves 34: a 40 GiB step
        // would be refused its writes.
        let quota = Room {
            shm_quota: Some(34 * GIB),
            ..room(100, 52)
        };
        assert!(!auto_fits(quota, 40 * GIB));
        assert!(auto_fits(quota, 30 * GIB));
        // A quota past the free space: the free space decides.
        let loose = Room {
            shm_quota: Some(500 * GIB),
            ..room(100, 52)
        };
        assert!(!auto_fits(loose, 60 * GIB));
    }

    const MOUNTINFO: &str = "\
22 1 0:21 / / rw,relatime shared:1 - btrfs /dev/sda2 rw,space_cache=v2
43 22 0:27 / /dev/shm rw,nosuid,nodev shared:3 - tmpfs tmpfs rw,inode64,usrquota
44 22 0:28 / /run rw,nosuid - tmpfs tmpfs rw,mode=755
45 22 0:29 / /srv/q rw - ext4 /dev/sdb1 rw,grpjquota=aquota.group,jqfmt=vfsv0
46 22 0:30 / /srv/p rw - xfs /dev/sdc1 rw,prjquota
47 22 0:31 / /mnt/my\\040disk rw - tmpfs tmpfs rw
";

    #[test]
    fn a_mount_s_quotas_are_read_from_its_options() {
        let at = |path: &str| mount_quotas(MOUNTINFO, Path::new(path)).expect("a mount");
        assert_eq!(
            at("/dev/shm/gaia-1/x"),
            (
                PathBuf::from("/dev/shm"),
                QuotaKinds {
                    user: true,
                    ..QuotaKinds::default()
                }
            )
        );
        assert_eq!(at("/run/user").1, QuotaKinds::default());
        assert_eq!(at("/home").0, PathBuf::from("/"));
        assert!(at("/srv/q/a").1.group);
        assert!(at("/srv/p").1.project);
        assert_eq!(at("/mnt/my disk/x").0, PathBuf::from("/mnt/my disk"));
    }

    #[test]
    fn a_quota_report_gives_the_least_headroom_of_its_lines() {
        let report = "\
Disk quotas for user sozo (uid 1000):
     Filesystem  blocks   quota   limit   grace   files   quota   limit   grace
tmpfs /dev/shm 17772784  52705190 52705190       0  250878       0       0       0
";
        assert_eq!(
            quota_report_headroom(report, Path::new("/dev/shm")).expect("reads"),
            Some((52_705_190 - 17_772_784) * 1024)
        );
        // Over the soft limit, with a grace: marked with a star.
        let over = "tmpfs /dev/shm 60000*  50000 70000  3days 1 0 0 0\n\
                    tmpfs /dev/shm 10 0 0 0 1 0 0 0\n";
        assert_eq!(
            quota_report_headroom(over, Path::new("/dev/shm")).expect("reads"),
            Some(0)
        );
        // No limit set for the id.
        let unlimited = "tmpfs /dev/shm 10 0 0 0 1 0 0 0\n";
        assert_eq!(
            quota_report_headroom(unlimited, Path::new("/dev/shm")).expect("reads"),
            None
        );
    }

    #[test]
    fn a_quota_report_without_the_mount_or_with_a_bad_number_is_an_error() {
        let other = "tmpfs /run 10 100 100 0 1 0 0 0\n";
        assert!(quota_report_headroom(other, Path::new("/dev/shm")).is_err());
        let bad = "tmpfs /dev/shm ten 100 100 0 1 0 0 0\n";
        let error = quota_report_headroom(bad, Path::new("/dev/shm"))
            .expect_err("does not read")
            .to_string();
        assert!(error.contains("blocks used"), "{error}");
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    const SHM_LINE: &str = "tmpfs /dev/shm 100 1100 1100 0 1 0 0 0\n";

    #[test]
    fn a_mount_with_no_quota_on_is_not_asked() {
        let asked = std::cell::Cell::new(false);
        let headroom = quota_headroom_with(Path::new("/run/x"), MOUNTINFO, |_, _| {
            asked.set(true);
            Ok(output(0, "", ""))
        });
        assert_eq!(headroom.expect("no quota"), None);
        assert!(!asked.get());
    }

    #[test]
    fn a_quota_that_is_on_is_read() {
        let headroom =
            quota_headroom_with(Path::new("/dev/shm/gaia-1"), MOUNTINFO, |flag, mount| {
                assert_eq!((flag, mount), ("-u", Path::new("/dev/shm")));
                Ok(output(0, SHM_LINE, ""))
            });
        assert_eq!(headroom.expect("reads"), Some(1000 * 1024));
    }

    #[test]
    fn a_quota_that_is_on_and_cannot_be_read_is_an_error_that_names_it() {
        let shm = Path::new("/dev/shm/gaia-1");
        let missing = quota_headroom_with(shm, MOUNTINFO, |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .expect_err("no tool")
        .to_string();
        assert!(missing.contains("`quota` could not be run"), "{missing}");
        let failed = quota_headroom_with(shm, MOUNTINFO, |_, _| {
            Ok(output(
                1,
                "",
                "quota: Not all specified mountpoints are using quota.",
            ))
        })
        .expect_err("tool failed")
        .to_string();
        assert!(failed.contains("Not all specified mountpoints"), "{failed}");
        let project = quota_headroom_with(Path::new("/srv/p/x"), MOUNTINFO, |_, _| {
            Ok(output(0, "", ""))
        })
        .expect_err("project quota")
        .to_string();
        assert!(project.contains("project quotas"), "{project}");
    }

    /// This machine's own: run with `--ignored` to see what placement reads.
    #[test]
    #[ignore = "reads this machine's /dev/shm quota"]
    fn this_machine_s_shm_quota_reads() {
        let headroom = quota_headroom(Path::new(SHM)).expect("reads");
        println!("/dev/shm quota headroom: {headroom:?}");
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
