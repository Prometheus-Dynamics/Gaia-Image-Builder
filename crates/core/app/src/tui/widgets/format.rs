//! Text for durations, ETAs, deltas, sizes and percentages, the same way
//! everywhere in the TUI.

/// A duration as `850ms`, `4.2s`, `42s`, `3m05s` or `1h02m`.
pub(crate) fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 10 {
        return format!("{:.1}s", ms as f64 / 1_000.0);
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3_600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3_600, (secs % 3_600) / 60)
}

/// A time left, marked as an estimate: `~8m12s`.
pub(crate) fn eta(ms: u64) -> String {
    format!("~{}", duration(ms))
}

/// A difference from the expected time: `+1m02s` (slower), `-4s` (faster).
pub(crate) fn delta(ms: i64) -> String {
    let sign = if ms >= 0 { '+' } else { '-' };
    format!("{sign}{}", duration(ms.unsigned_abs()))
}

/// A byte count in binary units: `812 B`, `3.4 MiB`, `1.2 GiB`.
pub(crate) fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if count < 1_024 {
        return format!("{count} B");
    }
    let mut value = count as f64 / 1_024.0;
    let mut unit = 0;
    while value >= 1_024.0 && unit < UNITS.len() - 1 {
        value /= 1_024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A rate in bytes per second: `3.4 MiB/s`.
pub(crate) fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes(bytes_per_sec.max(0.0) as u64))
}

/// A percentage from a fraction 0..1 or from a percent value, rounded.
pub(crate) fn percent(value: f64) -> String {
    format!("{:.0}%", value)
}

/// The clock of a unix time in milliseconds, UTC: `14:02 UTC`.
pub(crate) fn clock_utc(unix_ms: u64) -> String {
    let minutes = (unix_ms / 60_000) % (24 * 60);
    format!("{:02}:{:02} UTC", minutes / 60, minutes % 60)
}

/// `text` cut to `width` characters, the last one an ellipsis when cut.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_at_a_glance() {
        assert_eq!(duration(850), "850ms");
        assert_eq!(duration(4_200), "4.2s");
        assert_eq!(duration(42_000), "42s");
        assert_eq!(duration(185_000), "3m05s");
        assert_eq!(duration(3_720_000), "1h02m");
        assert_eq!(eta(492_000), "~8m12s");
        assert_eq!(delta(62_000), "+1m02s");
        assert_eq!(delta(-4_000), "-4.0s");
        assert_eq!(delta(0), "+0ms");
    }

    #[test]
    fn sizes_and_rates_use_binary_units() {
        assert_eq!(bytes(812), "812 B");
        assert_eq!(bytes(1_536), "1.5 KiB");
        assert_eq!(bytes(3 * 1_024 * 1_024 + 400_000), "3.4 MiB");
        assert_eq!(bytes(1_288_490_189), "1.2 GiB");
        assert_eq!(rate(2.0 * 1_024.0 * 1_024.0), "2.0 MiB/s");
        assert_eq!(rate(-5.0), "0 B/s");
    }

    #[test]
    fn clock_and_truncation() {
        assert_eq!(clock_utc(0), "00:00 UTC");
        assert_eq!(clock_utc(50_400_000), "14:00 UTC");
        assert_eq!(truncate("artifact:cli", 20), "artifact:cli");
        assert_eq!(truncate("image:buildroot-long", 8), "image:b…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(percent(42.4), "42%");
    }
}
