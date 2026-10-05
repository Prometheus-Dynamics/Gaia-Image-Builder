//! Ctrl-C / SIGTERM during `gaia run` cancel the build instead of killing
//! Gaia outright.
//!
//! Build processes run in their own process groups, so a terminal's Ctrl-C
//! reaches only Gaia. Without a handler Gaia died at once and left those
//! processes, and any docker containers they started, running unattended.
//! Cancelling instead lets the executor stop each command and remove its
//! container. A second signal exits immediately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use gaia_exec::ExecutionCancellation;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

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
