//! What the kernel's build writes about who built it, where and when.
//!
//! Left to itself, Linux stamps `user@host` and the time of the build into
//! its version string (`uname -v`, `/proc/version`) and the initramfs it
//! links in. That puts the builder's login and machine name in every image,
//! and makes two builds of the same inputs differ. Every make gets these
//! instead, the same on every machine:
//!
//! - `KBUILD_BUILD_USER` and `KBUILD_BUILD_HOST`: [`USER`] and [`HOST`].
//! - `KBUILD_BUILD_TIMESTAMP`: the Buildroot source's own release time
//!   (`BR2_VERSION_EPOCH` in its `Makefile`), the date Buildroot exports as
//!   `SOURCE_DATE_EPOCH` when nothing sets one, written as `date` writes it in
//!   UTC. A source with no such line gives the epoch itself.
//!
//! Any other package that stamps a build date (BusyBox's banner, for one)
//! takes it from `SOURCE_DATE_EPOCH`, the same release time, with `TZ` UTC
//! for one that formats it in local time.
//!
//! These are Buildroot's own values under `BR2_REPRODUCIBLE`, without the
//! rest of what that option changes. Where the build runs is `place`.
use std::fs;
use std::path::Path;

/// The user the kernel says built it.
pub(crate) const USER: &str = "gaia";
/// The host the kernel says it was built on.
pub(crate) const HOST: &str = "gaia";

/// The environment for every make of the Buildroot source at `src`.
pub(crate) fn env(src: &Path) -> [(&'static str, String); 5] {
    let epoch = release_epoch(src);
    [
        ("KBUILD_BUILD_USER", USER.to_string()),
        ("KBUILD_BUILD_HOST", HOST.to_string()),
        ("KBUILD_BUILD_TIMESTAMP", date(epoch)),
        ("SOURCE_DATE_EPOCH", epoch.to_string()),
        ("TZ", "UTC".to_string()),
    ]
}

/// `BR2_VERSION_EPOCH` from the source's `Makefile`, or 0.
fn release_epoch(src: &Path) -> u64 {
    fs::read_to_string(src.join("Makefile"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (name, value) = line.split_once('=')?;
                (name.trim() == "BR2_VERSION_EPOCH")
                    .then(|| value.trim().parse().ok())
                    .flatten()
            })
        })
        .unwrap_or(0)
}

/// `seconds` after the epoch as `LC_ALL=C TZ=UTC date` prints it, such as
/// `Thu Sep  4 13:16:40 UTC 2026`. The kernel prints it as given, and its
/// initramfs reads it back with `date -d`.
fn date(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm), from 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{} {} {:>2} {:02}:{:02}:{:02} UTC {}",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        day,
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60,
        year
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_print_as_date_prints_them_in_utc() {
        assert_eq!(date(0), "Thu Jan  1 00:00:00 UTC 1970");
        assert_eq!(date(951_782_400), "Tue Feb 29 00:00:00 UTC 2000");
        assert_eq!(date(1_788_535_000), "Fri Sep  4 15:16:40 UTC 2026");
        assert_eq!(date(4_107_542_399), "Sun Feb 28 23:59:59 UTC 2100");
    }

    #[test]
    fn the_timestamp_is_the_sources_release_time() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let src = scratch.path();
        fs::write(
            src.join("Makefile"),
            "BR2_VERSION := 2026.08\nBR2_VERSION_EPOCH = 1788535000\n",
        )
        .expect("makefile");
        let stamps = env(src);
        assert_eq!(stamps[0], ("KBUILD_BUILD_USER", "gaia".to_string()));
        assert_eq!(stamps[1], ("KBUILD_BUILD_HOST", "gaia".to_string()));
        assert_eq!(
            stamps[2],
            (
                "KBUILD_BUILD_TIMESTAMP",
                "Fri Sep  4 15:16:40 UTC 2026".to_string()
            )
        );
        fs::write(src.join("Makefile"), "all:\n").expect("makefile");
        assert_eq!(env(src)[2].1, "Thu Jan  1 00:00:00 UTC 1970");
    }
}
