//! Live progress of a Buildroot `make`: packages finished out of the
//! config's packages, the ones being built, and an estimate of the time
//! left, reported every few seconds as `gaia_process::build_progress_message`
//! lines on the operation's log, which `gaia run` shows in its progress line.
//!
//! The estimate uses how long each package took the last time it was built
//! (kept in `.gaia-package-durations` from `build/build-time.log`, so it
//! survives a clean): the time the packages left took then, scaled by how
//! fast this run gets through packages compared with then. Packages with no
//! recorded duration count as the median of the recorded ones, and no estimate
//! is given while most of what is left is such guesses.
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Seconds each package took the last time it was built, one `name seconds`
/// line per package.
pub(crate) const PACKAGE_DURATIONS: &str = ".gaia-package-durations";

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
        let log_path = output_dir.join("build/build-time.log");
        let thread = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let clock = gaia_process::ActiveClock::start();
                let done_before = installed(&packages);
                let mut last = std::time::Instant::now() - INTERVAL;
                while !stop.load(Ordering::SeqCst) {
                    if last.elapsed() >= INTERVAL && !gaia_process::is_paused() {
                        last = std::time::Instant::now();
                        let log = fs::read_to_string(log_path.as_path()).unwrap_or_default();
                        let progress = snapshot(
                            &packages,
                            &previous,
                            &done_before,
                            clock.elapsed(),
                            &running_packages(&log),
                            now_epoch(),
                        );
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

/// Seconds a package without a recorded duration is assumed to take when no
/// package has one.
const UNKNOWN_DURATION: f64 = 30.0;

/// Progress now, `elapsed` (not counting pauses) after `make` started with
/// `done_before` already built. `active` maps each package with a step running
/// to when its earliest open step started (epoch seconds), and `now` is the
/// current time in the same unit.
///
/// Packages without a recorded duration count as the median of the config's
/// recorded ones. A package being built counts what it has left: its duration
/// minus the time it has been building, but at least a tenth of the duration.
/// No estimate is given while the recorded durations cover less than half of
/// what is left, since then most of it is guesses.
fn snapshot(
    packages: &[(String, PathBuf)],
    previous: &BTreeMap<String, f64>,
    done_before: &BTreeSet<String>,
    elapsed: Duration,
    active: &BTreeMap<String, f64>,
    now: f64,
) -> gaia_process::BuildProgress {
    let done = installed(packages);
    let median = median_duration(packages, previous);
    let duration = |name: &str| previous.get(name).copied().unwrap_or(median);
    // Last time's duration of what this run finished.
    let finished_now: f64 = done
        .difference(done_before)
        .map(|name| duration(name.as_str()))
        .sum();
    let left = packages
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !done.contains(*name))
        .collect::<Vec<_>>();
    let left_estimate: f64 = left.iter().map(|name| duration(name)).sum();
    let left_recorded: f64 = left.iter().filter_map(|name| previous.get(*name)).sum();
    let mostly_known = left_recorded * 2.0 >= left_estimate;
    let eta =
        (mostly_known && finished_now >= 30.0 && elapsed >= Duration::from_secs(60)).then(|| {
            // Last time's seconds of the packages not started, scaled to this
            // run's pace; packages being built count their time left as is.
            let speed = elapsed.as_secs_f64() / finished_now;
            let seconds: f64 = left
                .iter()
                .map(|name| match active.get(*name) {
                    Some(started) => {
                        let total = duration(name);
                        let building = (now - started).max(0.0);
                        (total - building).max(0.1 * total)
                    }
                    None => duration(name) * speed,
                })
                .sum();
            Duration::from_secs_f64(seconds)
        });
    gaia_process::BuildProgress {
        done: done.len(),
        total: packages.len(),
        active: active.keys().cloned().collect(),
        eta,
    }
}

/// The median of the recorded durations of `packages`, or
/// `UNKNOWN_DURATION` when none is recorded.
fn median_duration(packages: &[(String, PathBuf)], previous: &BTreeMap<String, f64>) -> f64 {
    let mut known = packages
        .iter()
        .filter_map(|(name, _)| previous.get(name).copied())
        .collect::<Vec<_>>();
    if known.is_empty() {
        return UNKNOWN_DURATION;
    }
    known.sort_by(|a, b| a.total_cmp(b));
    let middle = known.len() / 2;
    if known.len() % 2 == 0 {
        (known[middle - 1] + known[middle]) / 2.0
    } else {
        known[middle]
    }
}

/// Packages with a step running now: started in `build-time.log` and not
/// ended, each with when its earliest open step started (epoch seconds).
/// (Stamps cannot tell: most packages get their download stamp at once and
/// then wait for their dependencies.)
fn running_packages(log: &str) -> BTreeMap<String, f64> {
    let mut open = BTreeMap::<(String, String), f64>::new();
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
                open.remove(&key);
            }
            _ => {}
        }
    }
    let mut running = BTreeMap::<String, f64>::new();
    for ((package, _), started) in open {
        running
            .entry(package)
            .and_modify(|first| *first = first.min(started))
            .or_insert(started);
    }
    running
}

/// The current time in epoch seconds, as `running_packages` reads it.
fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or(0.0)
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

    /// Makes `output`'s package directories, installed or not, for `snapshot`.
    fn fixture(output: &Path, packages: &[(&str, bool)]) -> Vec<(String, PathBuf)> {
        let _ = fs::remove_dir_all(output);
        packages
            .iter()
            .map(|(name, installed)| {
                let dir = output.join("build").join(name);
                fs::create_dir_all(&dir).expect("dir");
                if *installed {
                    fs::write(dir.join(".stamp_installed"), "").expect("stamp");
                }
                (name.to_string(), dir)
            })
            .collect()
    }

    #[test]
    fn running_packages_start_at_their_earliest_open_step() {
        let log = "100:start:build: zlib\n\
                   105:start:extract: zlib\n\
                   110:end:build: zlib\n\
                   120:start:build: mesa3d\n\
                   121:start:build: mesa3d\n\
                   130:start:build: curl\n\
                   131:end:build: curl\n";
        // zlib's extract is open since 105; curl's only step ended.
        assert_eq!(
            running_packages(log),
            BTreeMap::from([("zlib".to_string(), 105.0), ("mesa3d".to_string(), 121.0)])
        );
        assert!(running_packages("100:start:build: zlib\n100:end:build: zlib\n").is_empty());
    }

    #[test]
    fn unknown_active_kernel_makes_the_estimate_large() {
        let output = std::env::temp_dir().join(format!("gaia-kernel-{}", std::process::id()));
        // zlib was built before, curl finished in this run (60 s last time),
        // linux is building with no record, mesa3d (600 s) and python (200 s)
        // are left. Median of the recorded durations: 130 s for linux.
        let packages = fixture(
            &output,
            &[
                ("zlib", true),
                ("curl", true),
                ("linux", false),
                ("mesa3d", false),
                ("python", false),
            ],
        );
        let previous = BTreeMap::from([
            ("zlib".to_string(), 10.0),
            ("curl".to_string(), 60.0),
            ("mesa3d".to_string(), 600.0),
            ("python".to_string(), 200.0),
        ]);
        let done_before = BTreeSet::from(["zlib".to_string()]);
        // Pace: 120 s here for 60 s last time, so twice as slow. mesa3d and
        // python count 2 * 800 = 1600 s. linux has been building 10 s, so
        // 130 - 10 = 120 s are left on it: 1720 s in all.
        let active = running_packages("1000:start:build: linux\n");
        let progress = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(120),
            &active,
            1010.0,
        );
        assert_eq!(progress.active, ["linux"]);
        assert_eq!(progress.eta, Some(Duration::from_secs(1720)));
        // Later on, linux has 5 s left, floored at a tenth of its 130 s.
        let later = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(120),
            &active,
            1125.0,
        );
        // 130 - 125 = 5 s, floored at 13 s.
        assert_eq!(later.eta, Some(Duration::from_secs(1613)));
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn no_estimate_when_most_of_what_is_left_is_a_guess() {
        let output = std::env::temp_dir().join(format!("gaia-guess-{}", std::process::id()));
        let packages = fixture(
            &output,
            &[
                ("zlib", true),
                ("curl", true),
                ("a", false),
                ("b", false),
                ("c", false),
                ("d", false),
            ],
        );
        // Recorded: zlib 10, curl 60 (finished now), d 30. Median 30, so a, b
        // and c are guesses: 90 s of 120 s left.
        let mut previous = BTreeMap::from([
            ("zlib".to_string(), 10.0),
            ("curl".to_string(), 60.0),
            ("d".to_string(), 30.0),
        ]);
        let done_before = BTreeSet::from(["zlib".to_string()]);
        let guessing = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(120),
            &BTreeMap::new(),
            0.0,
        );
        assert_eq!(guessing.eta, None);
        // With the rest recorded too, most of what is left is known.
        previous.extend([
            ("a".to_string(), 30.0),
            ("b".to_string(), 30.0),
            ("c".to_string(), 30.0),
        ]);
        let known = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(120),
            &BTreeMap::new(),
            0.0,
        );
        // 120 s left at 120 s for 60 s: twice as slow, so 240 s.
        assert_eq!(known.eta, Some(Duration::from_secs(240)));
        let _ = fs::remove_dir_all(output);
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
        // twice as fast. photonvision (300 s, not started) counts 150 s now;
        // mesa3d (300 s, started 100 s ago) has 200 s left.
        let done_before = BTreeSet::from(["zlib".to_string()]);
        let active =
            running_packages("1:start:build: mesa3d\n1:start:extract: zlib\n2:end:extract: zlib\n");
        assert_eq!(active, BTreeMap::from([("mesa3d".to_string(), 1.0)]));
        let progress = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(300),
            &active,
            101.0,
        );
        assert_eq!(progress.done, 2);
        assert_eq!(progress.total, 4);
        assert_eq!(progress.active, ["mesa3d"]);
        assert_eq!(progress.eta, Some(Duration::from_secs(350)));
        // Too early to estimate.
        let early = snapshot(
            &packages,
            &previous,
            &done_before,
            Duration::from_secs(20),
            &BTreeMap::new(),
            101.0,
        );
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
