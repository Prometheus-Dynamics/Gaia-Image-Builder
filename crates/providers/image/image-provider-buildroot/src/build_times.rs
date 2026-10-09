//! Where a Buildroot `make` spent its time, from the `build/build-time.log`
//! Buildroot appends a start and an end line to for every package step:
//! the slowest packages built in this run, and the time after the last
//! package finished (finalizing the target and host trees, stripping,
//! filesystem images and post-image scripts).

use super::*;

/// Packages reported as their own steps, slowest first.
const SLOWEST_PACKAGES: usize = 8;

/// Step time messages for a `make` that ran from `started` to `finished`.
pub(crate) fn buildroot_build_time_steps(
    output_dir: &Path,
    started: std::time::SystemTime,
    finished: std::time::SystemTime,
) -> Vec<String> {
    let Ok(log) = fs::read_to_string(output_dir.join("build/build-time.log")) else {
        return Vec::new();
    };
    let seconds = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default()
    };
    let (started, finished) = (seconds(started), seconds(finished));
    let mut open = BTreeMap::<(String, String), f64>::new();
    let mut totals = BTreeMap::<String, f64>::new();
    let mut last_end = None::<f64>;
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
        if time < started {
            continue;
        }
        let key = (step.trim().to_string(), package.trim().to_string());
        match kind.trim() {
            "start" => {
                open.insert(key, time);
            }
            "end" => {
                if let Some(start) = open.remove(&key) {
                    *totals.entry(key.1).or_default() += time - start;
                }
                last_end = Some(last_end.map_or(time, |last: f64| last.max(time)));
            }
            _ => {}
        }
    }
    let mut packages = totals.into_iter().collect::<Vec<_>>();
    packages.sort_by(|left, right| right.1.total_cmp(&left.1));
    let duration = |seconds: f64| Duration::from_secs_f64(seconds.max(0.0));
    let mut steps = packages
        .into_iter()
        .take(SLOWEST_PACKAGES)
        .map(|(package, seconds)| {
            gaia_process::step_time_message(
                &format!("buildroot package {package}"),
                duration(seconds),
            )
        })
        .collect::<Vec<_>>();
    steps.push(gaia_process::step_time_message(
        "buildroot finalize and images",
        duration(finished - last_end.unwrap_or(started)),
    ));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_times_and_finalize_come_from_this_runs_log_lines() {
        let output = std::env::temp_dir().join(format!(
            "gaia-build-times-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(output.join("build")).expect("build dir");
        fs::write(
            output.join("build/build-time.log"),
            "50.0:start:build               : old\n\
             60.0:end  :build               : old\n\
             100.0:start:build               : mesa3d\n\
             400.0:end  :build               : mesa3d\n\
             100.0:start:build               : zlib\n\
             110.0:end  :build               : zlib\n\
             400.0:start:install-target      : mesa3d\n\
             430.0:end  :install-target      : mesa3d\n",
        )
        .expect("log");
        let at = |seconds: u64| std::time::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(
            buildroot_build_time_steps(&output, at(90), at(1630)),
            [
                "step time: buildroot package mesa3d 330000ms",
                "step time: buildroot package zlib 10000ms",
                "step time: buildroot finalize and images 1200000ms",
            ]
        );
        let _ = fs::remove_dir_all(output);
    }
}
