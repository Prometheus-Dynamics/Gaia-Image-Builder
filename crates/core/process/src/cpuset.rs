//! The CPUs Gaia may run on, passed to the containers it starts. A docker
//! container is a child of the docker daemon, not of Gaia, so it does not
//! inherit Gaia's CPU affinity: under `taskset -c 2-31 gaia run ...` a
//! cargo build in a container ran on every core, and the cores kept free
//! for the desktop were not.

/// `["--cpuset-cpus", "<list>"]` when this process may run on fewer CPUs
/// than are online; nothing otherwise, or where the affinity is unknown.
pub(crate) fn docker_cpuset_args() -> Vec<String> {
    match restricted_cpus() {
        Some(cpus) => vec!["--cpuset-cpus".into(), cpu_list(&cpus)],
        None => Vec::new(),
    }
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

    /// Under `taskset -c 0` (as the test runner may be) the containers get
    /// that CPU; unrestricted, nothing is added.
    #[test]
    fn the_cpuset_follows_this_process_affinity() {
        let args = docker_cpuset_args();
        assert!(
            args.is_empty() || (args.len() == 2 && args[0] == "--cpuset-cpus"),
            "{args:?}"
        );
    }
}
