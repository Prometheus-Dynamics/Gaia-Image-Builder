//! Text bars: a fraction drawn as filled blocks, for gauges and the rows of
//! the step and package lists.

/// `width` cells, filled to `fraction` (0..1, clamped).
pub(crate) fn bar(width: usize, fraction: f64) -> String {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = ((fraction * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// The share of `value` in `max`, 0 when there is no maximum.
pub(crate) fn share(value: u64, max: u64) -> f64 {
    if max == 0 {
        0.0
    } else {
        value as f64 / max as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_to_the_fraction() {
        assert_eq!(bar(4, 0.5), "██░░");
        assert_eq!(bar(4, 0.0), "░░░░");
        assert_eq!(bar(4, 1.0), "████");
        assert_eq!(bar(4, 7.0), "████");
        assert_eq!(bar(4, f64::NAN), "░░░░");
        assert_eq!(bar(0, 0.5), "");
        assert_eq!(share(5, 0), 0.0);
        assert_eq!(share(1, 4), 0.25);
    }
}
