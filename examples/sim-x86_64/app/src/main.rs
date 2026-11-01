//! The example's app: logs the simulated board it runs on, then idles as a
//! systemd service would.

use std::{fs, thread, time::Duration};

fn main() {
    let board = fs::read_to_string("/etc/sim/board").unwrap_or_else(|_| "unknown\n".into());
    println!("sim-app 0.1.0 on board {}", board.trim());
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
