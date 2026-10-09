//! Pausing a build: while paused, every running command is stopped (its
//! process group gets SIGSTOP, its docker container is paused) and no
//! timeout or timing counts the time.
//!
//! The state is process-wide: [`request_pause`] and [`request_resume`] flip
//! it, and each command's poll loop stops or continues its own processes
//! when it sees the change. [`ActiveClock`] measures time spent not paused.
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static PAUSED: AtomicBool = AtomicBool::new(false);

/// Paused time before the current pause, and when the current one began.
static PAUSE_TIME: Mutex<(Duration, Option<Instant>)> = Mutex::new((Duration::ZERO, None));

/// Pauses every running command (and any started while paused).
pub fn request_pause() {
    let mut time = PAUSE_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if time.1.is_none() {
        time.1 = Some(Instant::now());
    }
    PAUSED.store(true, Ordering::SeqCst);
}

/// Continues every paused command.
pub fn request_resume() {
    let mut time = PAUSE_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(since) = time.1.take() {
        time.0 += since.elapsed();
    }
    PAUSED.store(false, Ordering::SeqCst);
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// Time spent paused since the process started, including a pause in
/// progress.
pub fn paused_total() -> Duration {
    let time = PAUSE_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    time.0 + time.1.map(|since| since.elapsed()).unwrap_or_default()
}

/// A stopwatch that does not count time spent paused.
#[derive(Debug, Clone, Copy)]
pub struct ActiveClock {
    start: Instant,
    paused_at_start: Duration,
}

impl ActiveClock {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
            paused_at_start: paused_total(),
        }
    }

    /// Time since the start, minus time spent paused.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed().saturating_sub(self.paused())
    }

    /// Time spent paused since the start.
    pub fn paused(&self) -> Duration {
        paused_total().saturating_sub(self.paused_at_start)
    }
}
