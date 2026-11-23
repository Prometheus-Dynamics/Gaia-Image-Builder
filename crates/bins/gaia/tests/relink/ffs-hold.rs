//! ffs-hold MOUNT hold|release: a FunctionFS function's user side, for the
//! board profile's re-enable test.
//!
//! Writes one vendor interface with a bulk IN and a bulk OUT endpoint to
//! MOUNT/ep0, opens MOUNT/ep1 and MOUNT/ep2, and prints each event the kernel
//! sends on ep0. `hold` keeps the endpoint files open whatever happens, as a
//! holder that was not told to let go does. `release` closes them on DISABLE
//! or UNBIND and opens them again on ENABLE, as one that was told does.
//! `eps` is a second process that holds copies of the endpoint files only,
//! with no ep0: it opens MOUNT/ep1 and MOUNT/ep2 and sleeps, as a process
//! the endpoint files were handed to does.
//! Each line starts `ffs-hold[MODE]:`. It runs until ep0 fails or it is
//! killed.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

const DESCRIPTORS_MAGIC_V2: u32 = 3;
const STRINGS_MAGIC: u32 = 2;
const HAS_FS_DESC: u32 = 1;
const HAS_HS_DESC: u32 = 2;

fn interface() -> [u8; 9] {
    // bLength, INTERFACE, number 0, alternate 0, 2 endpoints, vendor class, iInterface 1
    [9, 4, 0, 0, 2, 0xff, 0, 0, 1]
}

fn endpoint(address: u8, packet: u16) -> [u8; 7] {
    let size = packet.to_le_bytes();
    // bLength, ENDPOINT, address, bulk, wMaxPacketSize, bInterval
    [7, 5, address, 2, size[0], size[1], 0]
}

fn descriptors() -> Vec<u8> {
    let speed = |packet: u16| {
        let mut bytes = interface().to_vec();
        bytes.extend(endpoint(0x81, packet));
        bytes.extend(endpoint(0x02, packet));
        bytes
    };
    let (full, high) = (speed(64), speed(512));
    let mut body = Vec::new();
    body.extend((HAS_FS_DESC | HAS_HS_DESC).to_le_bytes());
    body.extend(3u32.to_le_bytes());
    body.extend(3u32.to_le_bytes());
    body.extend(full);
    body.extend(high);
    let mut out = DESCRIPTORS_MAGIC_V2.to_le_bytes().to_vec();
    out.extend(((body.len() + 8) as u32).to_le_bytes());
    out.extend(body);
    out
}

fn strings() -> Vec<u8> {
    let text = b"relink ffs\0";
    let mut body = Vec::new();
    body.extend(1u32.to_le_bytes()); // strings per language
    body.extend(1u32.to_le_bytes()); // languages
    body.extend(0x0409u16.to_le_bytes());
    body.extend(text);
    let mut out = STRINGS_MAGIC.to_le_bytes().to_vec();
    out.extend(((body.len() + 8) as u32).to_le_bytes());
    out.extend(body);
    out
}

fn event_name(kind: u8) -> &'static str {
    match kind {
        0 => "BIND",
        1 => "UNBIND",
        2 => "ENABLE",
        3 => "DISABLE",
        4 => "SETUP",
        5 => "SUSPEND",
        6 => "RESUME",
        _ => "?",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mount), Some(mode)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: ffs-hold MOUNT hold|release");
        std::process::exit(2);
    };
    let say = |text: String| println!("ffs-hold[{mode}]: {text}");
    if mode == "eps" {
        let mut held = Vec::new();
        for name in ["ep1", "ep2"] {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("{mount}/{name}"))
            {
                Ok(file) => held.push(file),
                Err(error) => say(format!("open {name}: {error}")),
            }
        }
        say(format!("{} endpoint files held, no ep0", held.len()));
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    let release = match mode.as_str() {
        "hold" => false,
        "release" => true,
        _ => {
            eprintln!("usage: ffs-hold MOUNT hold|release|eps");
            std::process::exit(2);
        }
    };
    let open = |name: &str| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("{mount}/{name}"))
    };
    let mut ep0 = match open("ep0") {
        Ok(file) => file,
        Err(error) => {
            say(format!("open {mount}/ep0: {error}"));
            std::process::exit(1);
        }
    };
    for (what, bytes) in [("descriptors", descriptors()), ("strings", strings())] {
        if let Err(error) = ep0.write_all(&bytes) {
            say(format!("write {what}: {error}"));
            std::process::exit(1);
        }
    }
    say("descriptors and strings written".into());
    let endpoints = |say: &dyn Fn(String)| -> Vec<File> {
        let mut files = Vec::new();
        for name in ["ep1", "ep2"] {
            match open(name) {
                Ok(file) => files.push(file),
                Err(error) => say(format!("open {name}: {error}")),
            }
        }
        say(format!("{} endpoint files open", files.len()));
        files
    };
    let mut held = endpoints(&say);
    loop {
        // struct usb_functionfs_event: 8 bytes of setup, the type, 3 of padding.
        let mut event = [0u8; 12];
        match ep0.read(&mut event) {
            Ok(12) => {}
            Ok(n) => {
                say(format!("ep0 gave {n} bytes: ending"));
                break;
            }
            Err(error) => {
                say(format!("read ep0: {error}: ending"));
                break;
            }
        }
        let name = event_name(event[8]);
        say(format!("event {name}"));
        if release {
            match name {
                "DISABLE" | "UNBIND" if !held.is_empty() => {
                    held.clear();
                    say("endpoint files closed".into());
                }
                "ENABLE" if held.is_empty() => held = endpoints(&say),
                _ => {}
            }
        }
    }
}
