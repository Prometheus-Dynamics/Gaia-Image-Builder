//! The stock host's USB/IP attach, for scripts/qemu-stock-host.sh's guest:
//! connects a vsock stream to the host (CID 2) on PORT, retrying each second
//! until the relay takes it, and gives it to vhci_hcd as a high-speed device
//! on port 0. There is no usbipd handshake: the far end is a usbip-vudc
//! that starts USB/IP on the stream as it is.
//!
//!   vhci-attach PORT
//!
//! Built with rustc alone (no crates) for a static musl binary.

use std::io::Write;
use std::os::raw::{c_int, c_uint, c_ushort, c_void};

const AF_VSOCK: c_int = 40;
const SOCK_STREAM: c_int = 1;
const VMADDR_CID_HOST: c_uint = 2;
const VHCI_ATTACH: &str = "/sys/devices/platform/vhci_hcd.0/attach";
/// USB_SPEED_HIGH.
const SPEED_HIGH: u32 = 3;
/// Bus 1, device 2: the id vhci_hcd reports for the device.
const DEVID: u32 = (1 << 16) | 2;

#[repr(C)]
struct SockaddrVm {
    svm_family: c_ushort,
    svm_reserved1: c_ushort,
    svm_port: c_uint,
    svm_cid: c_uint,
    svm_flags: u8,
    svm_zero: [u8; 3],
}

extern "C" {
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn connect(fd: c_int, addr: *const c_void, len: c_uint) -> c_int;
    fn close(fd: c_int) -> c_int;
}

fn main() {
    let port: c_uint = match std::env::args().nth(1).and_then(|p| p.parse().ok()) {
        Some(p) => p,
        None => {
            eprintln!("usage: vhci-attach PORT");
            std::process::exit(2);
        }
    };
    let addr = SockaddrVm {
        svm_family: AF_VSOCK as c_ushort,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: VMADDR_CID_HOST,
        svm_flags: 0,
        svm_zero: [0; 3],
    };
    let mut tries = 0u32;
    let fd = loop {
        // SAFETY: plain syscalls on a socket this loop owns; addr outlives them.
        let fd = unsafe { socket(AF_VSOCK, SOCK_STREAM, 0) };
        if fd < 0 {
            eprintln!("stock-host: no vsock: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
        let len = std::mem::size_of::<SockaddrVm>() as c_uint;
        // SAFETY: as above.
        if unsafe { connect(fd, (&addr as *const SockaddrVm).cast(), len) } == 0 {
            break fd;
        }
        // SAFETY: as above.
        unsafe { close(fd) };
        tries += 1;
        if tries % 30 == 1 {
            println!(
                "stock-host: waiting for the device on vsock port {port}: {}",
                std::io::Error::last_os_error()
            );
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    };
    println!("stock-host: linked on vsock port {port} after {tries} retries");
    let line = format!("0 {fd} {DEVID} {SPEED_HIGH}");
    match std::fs::OpenOptions::new()
        .write(true)
        .open(VHCI_ATTACH)
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        Ok(()) => println!("stock-host: attached to vhci_hcd port 0, high speed"),
        Err(e) => {
            println!("stock-host: vhci_hcd attach failed: {e}");
            std::process::exit(1);
        }
    }
    // vhci_hcd holds its own reference to the socket.
}
