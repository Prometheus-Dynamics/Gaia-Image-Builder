//! The stock host's forward, for scripts/qemu-stock-host.sh's guest: listens
//! on vsock PORT, and joins each stream the PC's relay opens to a TCP
//! connection to HOST:TCP-PORT (the board's listener on the gadget's
//! network), bytes both ways, until either side ends. One line per
//! connection, starting "stock-host: forward".
//!
//!   vsock-forward PORT HOST TCP-PORT
//!
//! Built with rustc alone (no crates) for a static musl binary.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::FromRawFd;
use std::os::raw::{c_int, c_uint, c_ushort, c_void};

const AF_VSOCK: c_int = 40;
const SOCK_STREAM: c_int = 1;
const SOCK_CLOEXEC: c_int = 0o2000000;
const VMADDR_CID_ANY: c_uint = u32::MAX;
const SHUT_WR: c_int = 1;

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
    fn bind(fd: c_int, addr: *const c_void, len: c_uint) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
    fn accept(fd: c_int, addr: *mut c_void, len: *mut c_uint) -> c_int;
    fn shutdown(fd: c_int, how: c_int) -> c_int;
}

fn fail(what: &str) -> ! {
    println!(
        "stock-host: forward: {what}: {}",
        std::io::Error::last_os_error()
    );
    std::process::exit(1);
}

/// Copies `from` to `to` until `from` ends, then ends `to`'s writing.
fn pump(mut from: impl Read, mut to: impl Write, end: impl FnOnce()) -> u64 {
    let copied = std::io::copy(&mut from, &mut to).unwrap_or(0);
    let _ = to.flush();
    end();
    copied
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(port), Some(host), Some(tcp)) = (
        args.get(1).and_then(|p| p.parse::<c_uint>().ok()),
        args.get(2).cloned(),
        args.get(3).and_then(|p| p.parse::<u16>().ok()),
    ) else {
        eprintln!("usage: vsock-forward PORT HOST TCP-PORT");
        std::process::exit(2);
    };
    let addr = SockaddrVm {
        svm_family: AF_VSOCK as c_ushort,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: VMADDR_CID_ANY,
        svm_flags: 0,
        svm_zero: [0; 3],
    };
    let len = std::mem::size_of::<SockaddrVm>() as c_uint;
    // SAFETY: plain syscalls on a socket this function owns; addr outlives them.
    let fd = unsafe { socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        fail("no vsock");
    }
    // SAFETY: as above.
    if unsafe { bind(fd, (&addr as *const SockaddrVm).cast(), len) } != 0 {
        fail("bind");
    }
    // SAFETY: as above.
    if unsafe { listen(fd, 16) } != 0 {
        fail("listen");
    }
    println!("stock-host: forward: vsock port {port} to {host}:{tcp}");
    let mut n = 0u64;
    loop {
        // SAFETY: as above; the peer's address is not wanted.
        let conn = unsafe { accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if conn < 0 {
            fail("accept");
        }
        n += 1;
        // SAFETY: conn is a new descriptor only this File owns.
        let pc = unsafe { File::from_raw_fd(conn) };
        let host = host.clone();
        std::thread::spawn(move || {
            let board = match TcpStream::connect((host.as_str(), tcp)) {
                Ok(board) => board,
                Err(e) => {
                    println!("stock-host: forward {n}: connect {host}:{tcp}: {e}");
                    return;
                }
            };
            println!("stock-host: forward {n}: open to {host}:{tcp}");
            let (Ok(pc_in), Ok(board_in)) = (pc.try_clone(), board.try_clone()) else {
                println!("stock-host: forward {n}: cannot clone the streams");
                return;
            };
            let board_out = board.try_clone().ok();
            let up = std::thread::spawn(move || {
                pump(pc_in, board, move || {
                    if let Some(b) = board_out {
                        let _ = b.shutdown(Shutdown::Write);
                    }
                })
            });
            // SAFETY: shutdown on the descriptor pc owns, which outlives this call.
            let down = pump(board_in, pc, move || unsafe {
                shutdown(conn, SHUT_WR);
            });
            let up = up.join().unwrap_or(0);
            println!("stock-host: forward {n}: closed, {up} bytes to the board, {down} from it");
        });
    }
}
