//! `k230-firstage`: writes and checks K230 first-stage images.
//!
//!   k230-firstage wrap IN OUT        IN is an SPL or a uImage; OUT is the
//!                                    528-byte header over the zero version word
//!                                    and IN (the image's own bytes, as the ROM reads)
//!   k230-firstage priv-gzip IN OUT   IN is raw data; OUT is a gzip stream with
//!                                    the priv CM marker (0x09)
//!   k230-firstage check IMAGE        parses IMAGE and prints its length and digest

use std::process::ExitCode;

/// The `uimage` subcommand's flags, in any order, and the two paths.
fn uimage_args(args: &[String]) -> Result<(gaia_k230_image::UImageFields<'_>, &str, &str), String> {
    let mut os = None;
    let mut kind = None;
    let mut comp = None;
    let mut load = None;
    let mut entry = None;
    let mut name = None;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if let Some(_) = flag.strip_prefix("--") {
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("{flag} needs a value"))?
                .as_str();
            match flag {
                "--os" => os = Some(value),
                "--type" => kind = Some(value),
                "--comp" => comp = Some(value),
                "--load" => load = Some(value),
                "--entry" => entry = Some(value),
                "--name" => name = Some(value),
                _ => return Err(format!("unknown flag {flag}")),
            }
            i += 2;
        } else {
            paths.push(flag);
            i += 1;
        }
    }
    let os = match os.ok_or("--os is required")? {
        "u-boot" => gaia_k230_image::OS_U_BOOT,
        "linux" => gaia_k230_image::OS_LINUX,
        other => return Err(format!("--os {other}: want u-boot or linux")),
    };
    let kind = match kind.ok_or("--type is required")? {
        "firmware" => gaia_k230_image::TYPE_FIRMWARE,
        "kernel" => gaia_k230_image::TYPE_KERNEL,
        other => return Err(format!("--type {other}: want firmware or kernel")),
    };
    let comp = match comp.ok_or("--comp is required")? {
        "gzip" => gaia_k230_image::COMP_GZIP,
        "none" => gaia_k230_image::COMP_NONE,
        other => return Err(format!("--comp {other}: want gzip or none")),
    };
    let number = |text: Option<&str>, what: &str| -> Result<u32, String> {
        let text = text.ok_or_else(|| format!("{what} is required"))?;
        let parsed = match text.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => text.parse::<u32>(),
        };
        parsed.map_err(|_| format!("{what} {text}: not a number"))
    };
    let fields = gaia_k230_image::UImageFields {
        os,
        arch: gaia_k230_image::ARCH_RISCV,
        kind,
        comp,
        load: number(load, "--load")?,
        entry: number(entry, "--entry")?,
        time: 0,
        name: name.ok_or("--name is required")?,
    };
    match paths.as_slice() {
        [input, output] => Ok((fields, input, output)),
        _ => Err("uimage wants IN and OUT".to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["wrap", input, output] => {
            std::fs::read(input)
                .map_err(|e| e.to_string())
                .and_then(|data| {
                    std::fs::write(output, gaia_k230_image::wrap_firstage(&data))
                        .map_err(|e| e.to_string())
                })
        }
        ["priv-gzip", input, output] => std::fs::read(input)
            .map_err(|e| e.to_string())
            .and_then(|raw| gaia_k230_image::priv_gzip(&raw).map_err(|e| e.to_string()))
            .and_then(|stream| std::fs::write(output, stream).map_err(|e| e.to_string())),
        [sub, rest @ ..] if *sub == "uimage" => {
            let owned: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            uimage_args(&owned).and_then(|(fields, input, output)| {
                let data = std::fs::read(input).map_err(|e| e.to_string())?;
                let image = gaia_k230_image::uimage(&fields, &data).map_err(|e| e.to_string())?;
                std::fs::write(output, image).map_err(|e| e.to_string())
            })
        }
        ["check", image] => std::fs::read(image)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
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
