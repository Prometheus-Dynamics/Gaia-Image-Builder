//! What the checks read from the kernel: its release, taken from the shipped
//! image, and the exports `Module.symvers` lists.

use super::module::{decompress, normalise_name};
use std::collections::HashMap;
use std::path::Path;

const RELEASE_MARKER: &[u8] = b"Linux version ";
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

/// Compressed payloads a self-decompressing kernel (an arm `zImage`, an x86
/// `bzImage`) carries after its decompressor: the magic and the tool that
/// reads it.
const PAYLOADS: &[(&[u8], &str)] = &[
    (&[0x1f, 0x8b, 0x08], "gzip"),
    (&[0xfd, b'7', b'z', b'X', b'Z', 0x00], "xz"),
    (&[0x28, 0xb5, 0x2f, 0xfd], "zstd"),
];

/// How many places with a payload's magic are tried, per payload kind.
const PAYLOAD_TRIES: usize = 8;

/// The kernel release a shipped kernel image was built as. The image is read
/// as shipped (gunzipped first when it is gzip), so the release is the one
/// the image carries, not the one of any build directory. An image whose
/// banner is compressed (a `zImage`) is read through its gzip, xz or zstd
/// payload.
pub(super) fn kernel_release(image: &Path) -> Result<String, String> {
    let bytes = std::fs::read(image).map_err(|error| format!("cannot read: {error}"))?;
    let bytes = if bytes.starts_with(GZIP_MAGIC) {
        decompress("gzip", image)?
    } else {
        bytes
    };
    release_in(&bytes)
        .or_else(|| payload_release(&bytes))
        .ok_or_else(|| {
            format!(
                "no \"Linux version <release>\" string in {}, nor in a gzip, xz or zstd payload in it",
                image.display()
            )
        })
}

/// The release in the first compressed payload of `bytes` that has one. A
/// payload is decompressed from its magic to the end of the image; what
/// follows the stream is ignored, as is the tool's exit status, since the
/// tools report that trailing data as an error.
fn payload_release(bytes: &[u8]) -> Option<String> {
    PAYLOADS.iter().find_map(|(magic, tool)| {
        bytes
            .windows(magic.len())
            .enumerate()
            .filter(|(_, window)| window == magic)
            .take(PAYLOAD_TRIES)
            .find_map(|(offset, _)| release_in(&decompress_stream(tool, &bytes[offset..])?))
    })
}

/// What `tool -dc` makes of `input` on its stdin, or `None` when the tool
/// cannot be run.
fn decompress_stream(tool: &str, input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(tool)
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    // Written from a thread: the tool's output fills the pipe while it reads.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();
    Some(output.stdout)
}

/// The file in `images/` the Buildroot config's kernel format makes, as
/// Buildroot's `linux.mk` names it (`LINUX_IMAGE_NAME`). `Image` when the
/// config names no format.
pub(super) fn buildroot_kernel_image_name(config: &str) -> String {
    let value = |key: &str| {
        config.lines().find_map(|line| {
            line.trim()
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .map(|value| value.trim_matches('"').to_string())
        })
    };
    let set = |key: &str| value(key).as_deref() == Some("y");
    let first_dts = || {
        value("BR2_LINUX_KERNEL_INTREE_DTS_NAME")
            .and_then(|names| names.split_whitespace().next().map(str::to_string))
            .unwrap_or_default()
    };
    if set("BR2_LINUX_KERNEL_IMAGE_TARGET_CUSTOM") {
        return value("BR2_LINUX_KERNEL_IMAGE_NAME")
            .filter(|name| !name.is_empty())
            .or_else(|| value("BR2_LINUX_KERNEL_IMAGE_TARGET_NAME"))
            .unwrap_or_else(|| "Image".to_string());
    }
    let named = [
        ("BR2_LINUX_KERNEL_UIMAGE", "uImage"),
        ("BR2_LINUX_KERNEL_APPENDED_UIMAGE", "uImage"),
        ("BR2_LINUX_KERNEL_BZIMAGE", "bzImage"),
        ("BR2_LINUX_KERNEL_ZIMAGE", "zImage"),
        ("BR2_LINUX_KERNEL_ZIMAGE_EPAPR", "zImage.epapr"),
        ("BR2_LINUX_KERNEL_APPENDED_ZIMAGE", "zImage"),
        ("BR2_LINUX_KERNEL_IMAGE", "Image"),
        ("BR2_LINUX_KERNEL_IMAGEGZ", "Image.gz"),
        ("BR2_LINUX_KERNEL_LINUX_BIN", "linux.bin"),
        ("BR2_LINUX_KERNEL_VMLINUX_BIN", "vmlinux.bin"),
        ("BR2_LINUX_KERNEL_VMLINUX", "vmlinux"),
    ];
    if set("BR2_LINUX_KERNEL_CUIMAGE") {
        return format!("cuImage.{}", first_dts());
    }
    if set("BR2_LINUX_KERNEL_SIMPLEIMAGE") {
        return format!("simpleImage.{}", first_dts());
    }
    named
        .iter()
        .find(|(key, _)| set(key))
        .map_or("Image", |(_, name)| name)
        .to_string()
}

/// The first `Linux version <release> ` in `bytes` whose release starts with a
/// digit. The banner is in the image as plain text.
fn release_in(bytes: &[u8]) -> Option<String> {
    let first = RELEASE_MARKER[0];
    let mut from = 0;
    while let Some(found) = bytes.get(from..)?.iter().position(|byte| *byte == first) {
        let start = from + found;
        from = start + 1;
        let Some(rest) = bytes.get(start..start + RELEASE_MARKER.len()) else {
            continue;
        };
        if rest != RELEASE_MARKER {
            continue;
        }
        let token: Vec<u8> = bytes[start + RELEASE_MARKER.len()..]
            .iter()
            .take(96)
            .take_while(|byte| byte.is_ascii_graphic())
            .copied()
            .collect();
        if token.first().is_some_and(u8::is_ascii_digit) {
            return String::from_utf8(token).ok();
        }
    }
    None
}

/// One export of `Module.symvers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Export {
    pub crc: u32,
    /// `vmlinux`, or the normalised name of the exporting module.
    pub module: String,
}

/// The exports of a kernel build, from its `Module.symvers`.
#[derive(Debug, Default)]
pub(super) struct Symvers {
    exports: HashMap<String, Export>,
}

impl Symvers {
    /// Parses `Module.symvers` text. Lines that are not
    /// `<crc>\t<symbol>\t<module>\t...` are skipped.
    pub(super) fn parse(text: &str) -> Self {
        let mut exports = HashMap::new();
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let (Some(crc), Some(symbol), Some(module)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Some(crc) = crc
                .strip_prefix("0x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            else {
                continue;
            };
            let module = module.rsplit('/').next().unwrap_or(module);
            exports.entry(symbol.to_string()).or_insert(Export {
                crc,
                module: if module == "vmlinux" {
                    module.to_string()
                } else {
                    normalise_name(module)
                },
            });
        }
        Self { exports }
    }

    pub(super) fn get(&self, symbol: &str) -> Option<&Export> {
        self.exports.get(symbol)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.exports.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_is_the_first_banner_with_a_version_number() {
        let bytes = b"\0Linux version lots of words\0Linux version 7.2.9-v8-16k (builder) #1\0";
        assert_eq!(release_in(bytes).as_deref(), Some("7.2.9-v8-16k"));
        assert_eq!(release_in(b"no banner here"), None);
    }

    #[test]
    fn a_zimage_release_is_read_from_its_compressed_payload() {
        let compressed = std::process::Command::new("gzip")
            .arg("-c")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(b"\0Linux version 5.10.160 (builder) #1\0")?;
                child.wait_with_output()
            })
            .expect("gzip")
            .stdout;
        // A decompressor stub, a stray magic, the payload and trailing data
        // (the size word and the appended dtb of a zImage).
        let mut image = b"\x00decompressor\x1f\x8b\x08junk".to_vec();
        image.extend_from_slice(&compressed);
        image.extend_from_slice(b"\x10\x00\x00\x00\xd0\x0d\xfe\xed");
        let dir = gaia_testkit::TempDir::new("zimage");
        let path = dir.path().join("zImage");
        std::fs::write(&path, &image).expect("zImage");
        assert_eq!(kernel_release(&path).as_deref(), Ok("5.10.160"));
        std::fs::write(&path, b"no banner and no payload").expect("plain");
        assert!(kernel_release(&path).is_err());
    }

    #[test]
    fn the_kernel_image_name_follows_the_buildroot_format() {
        assert_eq!(buildroot_kernel_image_name(""), "Image");
        assert_eq!(
            buildroot_kernel_image_name("BR2_LINUX_KERNEL=y\nBR2_LINUX_KERNEL_ZIMAGE=y\n"),
            "zImage"
        );
        assert_eq!(
            buildroot_kernel_image_name("BR2_LINUX_KERNEL_IMAGEGZ=y\n"),
            "Image.gz"
        );
        assert_eq!(
            buildroot_kernel_image_name(
                "BR2_LINUX_KERNEL_SIMPLEIMAGE=y\nBR2_LINUX_KERNEL_INTREE_DTS_NAME=\"board other\"\n"
            ),
            "simpleImage.board"
        );
        assert_eq!(
            buildroot_kernel_image_name(
                "BR2_LINUX_KERNEL_IMAGE_TARGET_CUSTOM=y\nBR2_LINUX_KERNEL_IMAGE_TARGET_NAME=\"Image.lz4\"\nBR2_LINUX_KERNEL_IMAGE_NAME=\"\"\n"
            ),
            "Image.lz4"
        );
    }

    #[test]
    fn symvers_lines_parse_to_crc_and_exporter() {
        let symvers = Symvers::parse(
            "0xf7370f56\tsystem_state\tvmlinux\tEXPORT_SYMBOL\t\n\
             0x00000001\tv4l2_thing\tdrivers/media/v4l2-core/videodev\tEXPORT_SYMBOL_GPL\tNS\n\
             garbage line\n",
        );
        assert_eq!(symvers.len(), 2);
        assert_eq!(
            symvers.get("system_state"),
            Some(&Export {
                crc: 0xf737_0f56,
                module: "vmlinux".into()
            })
        );
        assert_eq!(
            symvers
                .get("v4l2_thing")
                .map(|export| export.module.as_str()),
            Some("videodev")
        );
    }
}
