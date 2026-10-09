//! Ctrl-C / SIGTERM during `gaia run` cancel the build instead of killing
//! Gaia outright.
//!
//! Build processes run in their own process groups, so a terminal's Ctrl-C
//! reaches only Gaia. Without a handler Gaia died at once and left those
//! processes, and any docker containers they started, running unattended.
//! Cancelling instead lets the executor stop each command and remove its
//! container. A second signal exits immediately.
//!
//! Ctrl-Z (SIGTSTP) pauses the build instead: every running command is
//! stopped (see `gaia_process::request_pause`), then Gaia stops itself so the
//! shell gets its prompt back; `fg` (SIGCONT) continues the commands.
//! `gaia pause|resume|cancel` send the same signals to the pid a run
//! publishes in `<build_dir>/.gaia-run.pid`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use gaia_exec::ExecutionCancellation;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The file a running `gaia run` keeps its pid in, under the build dir.
pub(crate) const RUN_PID_FILE: &str = ".gaia-run.pid";

#[cfg(unix)]
extern "C" fn on_pause_signal(_signal: libc::c_int) {
    PAUSE_REQUESTED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
extern "C" fn on_signal(_signal: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::SeqCst) {
        // SAFETY: `_exit` is async-signal-safe.
        unsafe { libc::_exit(130) };
    }
}

#[cfg(unix)]
fn install_handlers() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: the handler only touches an atomic and calls `_exit`,
            // both async-signal-safe.
            unsafe {
                libc::signal(signal, on_signal as *const () as libc::sighandler_t);
            }
        }
        // SAFETY: the handler only stores to an atomic.
        unsafe {
            libc::signal(
                libc::SIGTSTP,
                on_pause_signal as *const () as libc::sighandler_t,
            );
        }
    });
}

#[cfg(not(unix))]
fn install_handlers() {}

/// Cancels `cancellation` when Ctrl-C or SIGTERM arrives, until the returned
/// guard is dropped.
pub(crate) fn cancel_on_interrupt(cancellation: &ExecutionCancellation) -> InterruptGuard {
    install_handlers();
    INTERRUPTED.store(false, Ordering::SeqCst);
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let done = done.clone();
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if PAUSE_REQUESTED.swap(false, Ordering::SeqCst) {
                    pause_until_continued();
                }
                if INTERRUPTED.load(Ordering::SeqCst) {
                    eprintln!(
                        "gaia: interrupted; stopping running commands and their containers \
                         (press Ctrl-C again to exit immediately)"
                    );
                    cancellation.cancel();
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        })
    };
    InterruptGuard {
        done,
        watcher: Some(watcher),
    }
}

pub(crate) struct InterruptGuard {
    done: Arc<AtomicBool>,
    watcher: Option<thread::JoinHandle<()>>,
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

/// Pauses every running command, stops Gaia itself until SIGCONT (`fg`,
/// `gaia resume`, `gaia cancel`), then continues the commands.
#[cfg(unix)]
fn pause_until_continued() {
    gaia_process::request_pause();
    // Each command's poll loop stops its processes within milliseconds.
    thread::sleep(Duration::from_millis(200));
    eprintln!("gaia: paused; resume with `fg` or `gaia resume`, stop with `gaia cancel`");
    // SAFETY: signalling this process; SIGSTOP cannot be caught, and the call
    // returns once something sends SIGCONT.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGSTOP);
    }
    gaia_process::request_resume();
    eprintln!("gaia: resumed");
}

#[cfg(not(unix))]
fn pause_until_continued() {}

/// The pid in `pid_file`, when it is a live process named gaia.
#[cfg(unix)]
pub(crate) fn running_gaia(pid_file: &std::path::Path) -> Option<libc::pid_t> {
    let pid = std::fs::read_to_string(pid_file)
        .ok()?
        .trim()
        .parse::<libc::pid_t>()
        .ok()?;
    gaia_process_alive(pid)
}

/// `pid` when it is a live process named gaia. A pid since reused by another
/// program is not one.
#[cfg(unix)]
pub(crate) fn gaia_process_alive(pid: libc::pid_t) -> Option<libc::pid_t> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    (comm.is_empty() || comm.trim().starts_with("gaia")).then_some(pid)
}

/// Publishes this run's pid in `<build_dir>/.gaia-run.pid` until dropped.
pub(crate) fn publish_run(build_dir: &std::path::Path) -> RunPidGuard {
    let path = build_dir.join(RUN_PID_FILE);
    let written = std::fs::create_dir_all(build_dir)
        .and_then(|()| std::fs::write(&path, std::process::id().to_string()))
        .is_ok();
    RunPidGuard {
        path: written.then_some(path),
    }
}

pub(crate) struct RunPidGuard {
    path: Option<std::path::PathBuf>,
}

impl Drop for RunPidGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}
