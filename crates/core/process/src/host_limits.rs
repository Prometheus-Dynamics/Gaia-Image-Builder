//! The limits Gaia runs under, passed to the containers it starts. A docker
//! container is a child of the docker daemon, not of Gaia, so it inherits
//! none of them: under `taskset -c 2-31 gaia run ...` a cargo build in a
//! container ran on every core, and the cores kept free for the desktop
//! were not. The same held for `nice`, `ionice -c3` and a memory cap on
//! Gaia's cgroup (`systemd-run --user --scope -p MemoryMax=...`).
//!
//! - CPU affinity narrower than the online CPUs: `--cpuset-cpus`.
//! - A lowest memory cap on Gaia's cgroup or a parent: `--memory`.
//! - A positive nice value: `--cpu-shares`, the kernel's weight for it.
//! - The idle I/O class: `--blkio-weight 10`, the lowest (used by BFQ).

/// The `docker run` flags for the limits this process runs under.
pub(crate) fn docker_limit_args() -> Vec<String> {
    let mut args = Vec::new();
    if let Some(cpus) = restricted_cpus() {
        args.extend(["--cpuset-cpus".to_string(), cpu_list(&cpus)]);
    }
    if let Some(bytes) = memory_cap() {
        args.extend(["--memory".to_string(), bytes.to_string()]);
    }
    if let Some(shares) = cpu_shares_for_nice(own_nice()) {
        args.extend(["--cpu-shares".to_string(), shares.to_string()]);
    }
    if io_class_is_idle() {
        args.extend(["--blkio-weight".to_string(), "10".to_string()]);
    }
    args
}

/// The kernel's CPU weight for a nice value (`sched_prio_to_weight`,
/// nice 0 = 1024), as docker's shares; `None` for nice 0 or less.
pub(crate) fn cpu_shares_for_nice(nice: i32) -> Option<u32> {
    const WEIGHTS: [u32; 19] = [
        820, 655, 526, 423, 335, 272, 215, 172, 137, 110, 87, 70, 56, 45, 36, 29, 23, 18, 15,
    ];
    let index = usize::try_from(nice).ok()?.checked_sub(1)?;
    Some(WEIGHTS[index.min(WEIGHTS.len() - 1)])
}

/// The lowest `memory.max` of `cgroup` and its parents under `root`.
pub(crate) fn lowest_memory_max(root: &std::path::Path, cgroup: &str) -> Option<u64> {
    let mut lowest: Option<u64> = None;
    let mut path = std::path::PathBuf::from(cgroup.trim_start_matches('/'));
    loop {
        if let Ok(text) = std::fs::read_to_string(root.join(&path).join("memory.max"))
            && let Ok(bytes) = text.trim().parse::<u64>()
        {
            lowest = Some(lowest.map_or(bytes, |current| current.min(bytes)));
        }
        if !path.pop() {
            break;
        }
    }
    lowest
}

#[cfg(target_os = "linux")]
fn memory_cap() -> Option<u64> {
    let cgroups = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let cgroup = cgroups.lines().find_map(|line| line.strip_prefix("0::"))?;
    lowest_memory_max(std::path::Path::new("/sys/fs/cgroup"), cgroup)
}

#[cfg(target_os = "linux")]
fn own_nice() -> i32 {
    // SAFETY: getpriority reads this process's nice value. -1 is also a
    // valid value, so errno tells an error apart; either way it is <= 0.
    unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) }
}

#[cfg(target_os = "linux")]
fn io_class_is_idle() -> bool {
    const IOPRIO_WHO_PROCESS: libc::c_long = 1;
    const IOPRIO_CLASS_IDLE: libc::c_long = 3;
    // SAFETY: ioprio_get only reads this process's I/O priority.
    let priority = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) };
    priority >= 0 && priority >> 13 == IOPRIO_CLASS_IDLE
}

#[cfg(not(target_os = "linux"))]
fn memory_cap() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
fn own_nice() -> i32 {
    0
}

#[cfg(not(target_os = "linux"))]
fn io_class_is_idle() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn restricted_cpus() -> Option<Vec<usize>> {
    // SAFETY: `set` is a plain bit set that sched_getaffinity fills in;
    // the size passed is its own.
    let set = unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        set
    };
    let allowed = (0..libc::CPU_SETSIZE as usize)
        // SAFETY: `cpu` is below CPU_SETSIZE, the set's size.
        .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &set) })
        .collect::<Vec<_>>();
    // SAFETY: sysconf only reads a system value.
    let online = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    let online = usize::try_from(online).ok()?;
    (!allowed.is_empty() && allowed.len() < online).then_some(allowed)
}

#[cfg(not(target_os = "linux"))]
fn restricted_cpus() -> Option<Vec<usize>> {
    None
}

/// `2-5,8,10-11`: sorted CPU numbers as docker's `--cpuset-cpus` takes them.
pub(crate) fn cpu_list(cpus: &[usize]) -> String {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &cpu in cpus {
        match ranges.last_mut() {
            Some((_, end)) if *end + 1 == cpu => *end = cpu,
            _ => ranges.push((cpu, cpu)),
        }
    }
    ranges
        .iter()
        .map(|&(start, end)| {
            if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cpu_list_joins_runs_into_ranges() {
        assert_eq!(cpu_list(&[2, 3, 4, 5, 8, 10, 11]), "2-5,8,10-11");
        assert_eq!(cpu_list(&[0]), "0");
        assert_eq!(cpu_list(&[]), "");
    }

    /// Whatever limits the test runner has, the flags come in pairs.
    #[test]
    fn the_flags_follow_this_process_limits() {
        let args = docker_limit_args();
        assert_eq!(args.len() % 2, 0, "{args:?}");
        assert!(
            args.iter().step_by(2).all(|flag| flag.starts_with("--")),
            "{args:?}"
        );
    }

    #[test]
    fn a_nice_value_is_its_kernel_weight() {
        assert_eq!(cpu_shares_for_nice(0), None);
        assert_eq!(cpu_shares_for_nice(-5), None);
        assert_eq!(cpu_shares_for_nice(10), Some(110));
        assert_eq!(cpu_shares_for_nice(19), Some(15));
        assert_eq!(cpu_shares_for_nice(40), Some(15));
    }

    #[test]
    fn the_lowest_memory_cap_of_the_cgroup_and_its_parents_counts() {
        let root = gaia_testkit::TempDir::new("gaia-host-limits-cgroup");
        let put = |path: &str, value: &str| {
            let dir = root.path().join(path);
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(dir.join("memory.max"), value).expect("write");
        };
        put("user.slice", "8589934592\n");
        put("user.slice/app.scope", "max\n");
        assert_eq!(
            lowest_memory_max(root.path(), "/user.slice/app.scope"),
            Some(8_589_934_592)
        );
        put("user.slice/app.scope", "2147483648\n");
        assert_eq!(
            lowest_memory_max(root.path(), "/user.slice/app.scope"),
            Some(2_147_483_648)
        );
        assert_eq!(lowest_memory_max(root.path(), "/elsewhere"), None);
    }
}
