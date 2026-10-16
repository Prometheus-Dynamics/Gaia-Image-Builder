//! `${build.date}`: the build's reproducible time as RFC 3339 UTC
//! (`2026-10-15T12:00:00Z`), for a manifest's `built` field.
//!
//! The time is the caller's `SOURCE_DATE_EPOCH` when the environment sets
//! one, else the commit time of the git `HEAD` of the build file's repository
//! (the same rule the Buildroot provider uses for `make`). Outside both, the
//! token stays unresolved, so an assembly that uses it fails loudly instead
//! of writing a wall-clock time.

use std::path::Path;
use std::process::{Command, Stdio};

/// The RFC 3339 UTC time for the build, when one is known.
pub(super) fn resolve(build_file_dir: &str) -> Option<String> {
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| head_commit_time(build_file_dir))?;
    let seconds = epoch.parse::<i64>().ok()?;
    Some(rfc3339_utc(seconds))
}

fn head_commit_time(build_file_dir: &str) -> Option<String> {
    if build_file_dir.is_empty() {
        return None;
    }
    Command::new("git")
        .arg("-C")
        .arg(Path::new(build_file_dir))
        .args(["log", "-1", "--format=%ct", "HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
}

/// `seconds` since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn rfc3339_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        secs_of_day % 3600 / 60,
        secs_of_day % 60
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::rfc3339_utc;

    #[test]
    fn epochs_render_as_rfc3339_utc() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // A leap day, and the last second of a year.
        assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_704_067_199), "2023-12-31T23:59:59Z");
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59Z");
    }
}
