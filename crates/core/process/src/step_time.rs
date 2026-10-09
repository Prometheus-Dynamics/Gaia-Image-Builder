//! Wall time of the steps inside an operation (a Buildroot `make`, a
//! package cache restore, an assembly transform), reported as operation log
//! messages so every provider can add them without new plumbing; the
//! runtime collects them into the run's timings.

use std::time::Duration;

/// Prefix of the log messages that carry a step's wall time.
pub const STEP_TIME_PREFIX: &str = "step time: ";

/// `step time: <step> <milliseconds>ms`.
pub fn step_time_message(step: &str, duration: Duration) -> String {
    format!("{STEP_TIME_PREFIX}{step} {}ms", duration.as_millis())
}

/// The step and duration of a [`step_time_message`].
pub fn parse_step_time(message: &str) -> Option<(String, Duration)> {
    let rest = message.strip_prefix(STEP_TIME_PREFIX)?;
    let (step, millis) = rest.rsplit_once(' ')?;
    let millis = millis.strip_suffix("ms")?.parse::<u64>().ok()?;
    Some((step.to_string(), Duration::from_millis(millis)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_times_round_trip() {
        let message = step_time_message("buildroot make", Duration::from_millis(61_250));
        assert_eq!(message, "step time: buildroot make 61250ms");
        assert_eq!(
            parse_step_time(&message),
            Some(("buildroot make".to_string(), Duration::from_millis(61_250)))
        );
        assert_eq!(parse_step_time("built image"), None);
    }
}
