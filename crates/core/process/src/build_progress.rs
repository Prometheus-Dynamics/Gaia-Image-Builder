//! Progress inside a long operation (the packages of a Buildroot `make`),
//! reported as operation log messages like step times, so `gaia run` can
//! show it instead of the operation count alone.

use std::time::Duration;

/// Prefix of the log messages that carry an operation's inner progress.
pub const BUILD_PROGRESS_PREFIX: &str = "build progress: ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProgress {
    /// Units (packages) finished, including those finished before this run.
    pub done: usize,
    pub total: usize,
    /// Units in progress now.
    pub active: Vec<String>,
    /// Estimated time left, when it can be estimated.
    pub eta: Option<Duration>,
}

/// `build progress: <done>/<total> eta=<seconds|-> active=<a,b,...>`.
pub fn build_progress_message(progress: &BuildProgress) -> String {
    format!(
        "{BUILD_PROGRESS_PREFIX}{}/{} eta={} active={}",
        progress.done,
        progress.total,
        progress
            .eta
            .map(|eta| eta.as_secs().to_string())
            .unwrap_or_else(|| "-".to_string()),
        progress.active.join(",")
    )
}

/// The progress in a [`build_progress_message`].
pub fn parse_build_progress(message: &str) -> Option<BuildProgress> {
    let rest = message.strip_prefix(BUILD_PROGRESS_PREFIX)?;
    let mut fields = rest.splitn(3, ' ');
    let (done, total) = fields.next()?.split_once('/')?;
    let eta = fields.next()?.strip_prefix("eta=")?;
    let active = fields.next()?.strip_prefix("active=")?;
    Some(BuildProgress {
        done: done.parse().ok()?,
        total: total.parse().ok()?,
        active: active
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
        eta: match eta {
            "-" => None,
            seconds => Some(Duration::from_secs(seconds.parse().ok()?)),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_progress_round_trips() {
        let progress = BuildProgress {
            done: 120,
            total: 290,
            active: vec!["mesa3d".into(), "linux".into()],
            eta: Some(Duration::from_secs(2100)),
        };
        let message = build_progress_message(&progress);
        assert_eq!(
            message,
            "build progress: 120/290 eta=2100 active=mesa3d,linux"
        );
        assert_eq!(parse_build_progress(&message), Some(progress));
        let idle = BuildProgress {
            done: 0,
            total: 3,
            active: Vec::new(),
            eta: None,
        };
        assert_eq!(
            parse_build_progress(&build_progress_message(&idle)),
            Some(idle)
        );
        assert_eq!(parse_build_progress("step time: x 1ms"), None);
    }
}
