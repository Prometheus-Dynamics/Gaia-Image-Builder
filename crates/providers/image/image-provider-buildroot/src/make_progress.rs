//! Live progress of a Buildroot `make`: packages finished out of the
//! config's packages, the ones being built, and an estimate of the time
//! left, reported every few seconds as `gaia_process::build_progress_message`
//! lines on the operation's log, which `gaia run` shows in its progress line.
//!
//! The estimate uses how long each package took the last time it was built
//! (kept in `.gaia-package-durations` from `build/build-time.log`, so it
//! survives a clean): the time the packages left took then, scaled by how
//! fast this run gets through packages compared with then.
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Seconds each package took the last time it was built, one `name seconds`
/// line per package.
const PACKAGE_DURATIONS: &str = ".gaia-package-durations";

/// How often progress is reported.
const INTERVAL: Duration = Duration::from_secs(10);

/// Reports progress until dropped.
pub(crate) struct MakeProgress {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MakeProgress {
    /// Starts reporting for the `make` about to run in `output_dir`, with the
    /// package graph recorded for it; `None` without a graph or a log.
    pub(crate) fn start(
        output_dir: &Path,
        log_sink: Option<gaia_process::ProcessLogSink>,
    ) -> Option<Self> {
        let sink = log_sink?;
        let graph = PackageGraph::load(output_dir)?;
        let packages = graph
            .packages
            .iter()
            .filter_map(|(name, package)| {
                Some((name.clone(), output_dir.join(package.stamp_dir.as_ref()?)))
            })
            .collect::<Vec<_>>();
        let previous = load_package_durations(output_dir);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let clock = gaia_process::ActiveClock::start();
                let done_before = installed(&packages);
                let mut last = std::time::Instant::now() - INTERVAL;
                while !stop.load(Ordering::SeqCst) {
                    if last.elapsed() >= INTERVAL && !gaia_process::is_paused() {
                        last = std::time::Instant::now();
                        let progress =
                            snapshot(&packages, &previous, &done_before, clock.elapsed());
                        sink(gaia_process::ProcessLogLine {
                            stream: gaia_process::ProcessLogStream::Stdout,
                            line: gaia_process::build_progress_message(&progress),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
        };
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for MakeProgress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The packages built (with `.stamp_installed`).
fn installed(packages: &[(String, PathBuf)]) -> BTreeSet<String> {
    packages
        .iter()
        .filter(|(_, dir)| dir.join(".stamp_installed").exists())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Progress now, `elapsed` (not counting pauses) after `make` started with
/// `done_before` already built.
fn snapshot(
    packages: &[(String, PathBuf)],
    previous: &BTreeMap<String, f64>,
    done_before: &BTreeSet<String>,
    elapsed: Duration,
) -> gaia_process::BuildProgress {
    let done = installed(packages);
    let mut active = packages
        .iter()
        .filter(|(name, dir)| !done.contains(name) && in_progress(dir))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    active.sort();
    // Last time's duration of what this run finished, and of what is left.
    let finished_now = took(previous, done.difference(done_before));
    let left = took(
        previous,
        packages
            .iter()
            .map(|(name, _)| name)
            .filter(|name| !done.contains(*name)),
    );
    let eta = (finished_now >= 30.0 && elapsed >= Duration::from_secs(60))
        .then(|| Duration::from_secs_f64(left * elapsed.as_secs_f64() / finished_now));
    gaia_process::BuildProgress {
        done: done.len(),
        total: packages.len(),
        active,
        eta,
    }
}

/// How long `names` took last time, in seconds (unknown ones count 0).
fn took<'a>(previous: &BTreeMap<String, f64>, names: impl Iterator<Item = &'a String>) -> f64 {
    names.filter_map(|name| previous.get(name)).sum()
}

/// Seconds each package took the last time it was built.
fn load_package_durations(output_dir: &Path) -> BTreeMap<String, f64> {
    let mut durations = fs::read_to_string(output_dir.join(PACKAGE_DURATIONS))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (name, seconds) = line.rsplit_once(' ')?;
            Some((name.to_string(), seconds.parse::<f64>().ok()?))
        })
        .collect::<BTreeMap<_, _>>();
    // A tree built before the file existed still has its log.
    if let Ok(log) = fs::read_to_string(output_dir.join("build/build-time.log")) {
        durations.extend(last_package_durations(&log));
    }
    durations
}

/// After a `make`: keeps how long each package took, for the next estimate.
pub(crate) fn record_package_durations(output_dir: &Path) {
    let durations = load_package_durations(output_dir);
    if durations.is_empty() {
        return;
    }
    let contents = durations
        .iter()
        .map(|(name, seconds)| format!("{name} {seconds:.1}\n"))
        .collect::<String>();
    let _ = fs::write(output_dir.join(PACKAGE_DURATIONS), contents);
}

/// Each package's time in `build-time.log`, from the last time each of its
/// steps ran.
fn last_package_durations(log: &str) -> BTreeMap<String, f64> {
    let mut open = BTreeMap::<(String, String), f64>::new();
    let mut steps = BTreeMap::<(String, String), f64>::new();
    for line in log.lines() {
        // `<epoch seconds>:<start|end>:<step>: <package>`
        let mut parts = line.splitn(4, ':');
        let (Some(time), Some(kind), Some(step), Some(package)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(time) = time.parse::<f64>() else {
            continue;
        };
        let key = (package.trim().to_string(), step.trim().to_string());
        match kind.trim() {
            "start" => {
                open.insert(key, time);
            }
            "end" => {
                if let Some(start) = open.remove(&key) {
                    steps.insert(key, (time - start).max(0.0));
                }
            }
            _ => {}
        }
    }
    let mut packages = BTreeMap::<String, f64>::new();
    for ((package, _), seconds) in steps {
        *packages.entry(package).or_default() += seconds;
    }
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_come_from_the_last_build_of_each_step() {
        let log = "100:start:build: zlib\n\
                   110:end:build: zlib\n\
                   110:start:install-target: zlib\n\
                   112:end:install-target: zlib\n\
                   200:start:build: zlib\n\
                   204:end:build: zlib\n\
                   300:start:build: mesa3d\n";
        assert_eq!(
            last_package_durations(log),
            BTreeMap::from([("zlib".to_string(), 6.0)])
        );
    }

    #[test]
    fn progress_counts_packages_and_estimates_from_the_last_build() {
        let output = std::env::temp_dir().join(format!("gaia-progress-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);
        let mut packages = Vec::new();
        for (name, stamps) in [
            ("zlib", &[".stamp_built", ".stamp_installed"][..]),
            ("linux", &[".stamp_built", ".stamp_installed"][..]),
            ("mesa3d", &[".stamp_configured"][..]),
            ("photonvision", &[][..]),
        ] {
            let dir = output.join("build").join(name);
            fs::create_dir_all(&dir).expect("dir");
            for stamp in stamps {
                fs::write(dir.join(stamp), "").expect("stamp");
            }
            packages.push((name.to_string(), dir));
        }
        let previous = BTreeMap::from([
            ("zlib".to_string(), 10.0),
            ("linux".to_string(), 600.0),
            ("mesa3d".to_string(), 300.0),
            ("photonvision".to_string(), 300.0),
        ]);
        // zlib was built before; linux took 600 s last time and 300 s now:
        // twice as fast, so the 600 s left then are 300 s now.
        let done_before = BTreeSet::from(["zlib".to_string()]);
        let progress = snapshot(&packages, &previous, &done_before, Duration::from_secs(300));
        assert_eq!(progress.done, 2);
        assert_eq!(progress.total, 4);
        assert_eq!(progress.active, ["mesa3d"]);
        assert_eq!(progress.eta, Some(Duration::from_secs(300)));
        // Too early to estimate.
        let early = snapshot(&packages, &previous, &done_before, Duration::from_secs(20));
        assert_eq!(early.eta, None);

        fs::write(
            output.join("build/build-time.log"),
            "1:start:build: zlib\n13:end:build: zlib\n",
        )
        .expect("log");
        record_package_durations(&output);
        assert_eq!(
            fs::read_to_string(output.join(PACKAGE_DURATIONS)).expect("durations"),
            "zlib 12.0\n"
        );
        let _ = fs::remove_dir_all(output);
    }
}
