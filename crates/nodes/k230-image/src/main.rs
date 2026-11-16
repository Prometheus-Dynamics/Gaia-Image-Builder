//! `k230-firstage`: writes and checks K230 first-stage images.
//!
//!   k230-firstage wrap IN OUT        IN is an SPL or a uImage; OUT is the
//!                                    528-byte header over the zero version word
//!                                    and IN (the image's own bytes, as the ROM reads)
//!   k230-firstage priv-gzip IN OUT   IN is raw data; OUT is a gzip stream with
//!                                    the priv CM marker (0x09)
//!   k230-firstage check IMAGE        parses IMAGE and prints its length and digest

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["wrap", input, output] => std::fs::read(input)
            .map_err(|e| e.to_string())
            .and_then(|data| std::fs::write(output, gaia_k230_image::wrap_firstage(&data)).map_err(|e| e.to_string())),
        ["priv-gzip", input, output] => std::fs::read(input)
            .map_err(|e| e.to_string())
            .and_then(|raw| gaia_k230_image::priv_gzip(&raw).map_err(|e| e.to_string()))
            .and_then(|stream| std::fs::write(output, stream).map_err(|e| e.to_string())),
        ["check", image] => std::fs::read(image).map_err(|e| e.to_string()).and_then(|bytes| {
            let parsed = gaia_k230_image::parse_firstage(&bytes).map_err(|e| e.to_string())?;
            let hex: String = parsed.digest.iter().map(|b| format!("{b:02x}")).collect();
            println!("payload_len={} sha256={hex}", parsed.payload_len);
            Ok(())
        }),
        _ => Err("usage: k230-firstage wrap IN OUT | priv-gzip IN OUT | check IMAGE".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("k230-firstage: {message}");
            ExitCode::FAILURE
        }
    }
}
