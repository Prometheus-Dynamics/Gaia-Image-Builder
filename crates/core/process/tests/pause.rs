//! Pausing is process-wide, so these tests run in their own binary, one at a
//! time.
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Pauses for `pause` after `after`, on another thread.
fn pause_later(after: Duration, pause: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(after);
        gaia_process::request_pause();
        thread::sleep(pause);
        gaia_process::request_resume();
    })
}

#[test]
fn pausing_stops_commands_without_timing_them_out() {
    // 1. Paused past its timeout, a command still finishes.
    let pauser = pause_later(Duration::from_millis(100), Duration::from_millis(1500));
    let clock = gaia_process::ActiveClock::start();
    let started = Instant::now();
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 0.3; sleep 0.3"]);
    let result = gaia_process::run_command_with_timeout(
        &mut command,
        Duration::from_millis(1200),
        "paused sleep",
        None,
        None,
    );
    pauser.join().expect("pauser");
    assert!(result.is_ok(), "{result:?}");
    assert!(started.elapsed() >= Duration::from_millis(1500));
    assert!(
        clock.paused() >= Duration::from_millis(1400),
        "{:?}",
        clock.paused()
    );
    assert!(
        clock.elapsed() < Duration::from_millis(1200),
        "{:?}",
        clock.elapsed()
    );

    // 2. A paused command does not run: it writes nothing while paused.
    let marker = std::env::temp_dir().join(format!("gaia-pause-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let pauser = pause_later(Duration::from_millis(100), Duration::from_millis(1000));
    let watcher = {
        let marker = marker.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(700));
            marker.exists()
        })
    };
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!("sleep 0.2; : > '{}'", marker.display()));
    let result = gaia_process::run_command_with_timeout(
        &mut command,
        Duration::from_secs(10),
        "paused writer",
        None,
        None,
    );
    pauser.join().expect("pauser");
    assert!(result.is_ok(), "{result:?}");
    assert!(
        !watcher.join().expect("watcher"),
        "the command ran while paused"
    );
    assert!(marker.exists());
    let _ = std::fs::remove_file(&marker);

    // 3. Cancelling a paused command still stops it.
    let cancelled = Arc::new(AtomicBool::new(false));
    gaia_process::request_pause();
    let cancel = {
        let cancelled = cancelled.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            cancelled.store(true, Ordering::SeqCst);
        })
    };
    let mut command = Command::new("sleep");
    command.arg("30");
    let check = cancelled.clone();
    let started = Instant::now();
    let result = gaia_process::run_command_with_timeout(
        &mut command,
        Duration::from_secs(60),
        "paused and cancelled",
        None,
        Some(Arc::new(move || check.load(Ordering::SeqCst))),
    );
    gaia_process::request_resume();
    cancel.join().expect("cancel");
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(10));
}
