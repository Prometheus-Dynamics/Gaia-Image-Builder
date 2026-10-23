//! What the checks read from the kernel: its release, taken from the shipped
//! image, and the exports `Module.symvers` lists.

use super::module::{decompress, normalise_name};
use std::collections::HashMap;
use std::path::Path;

const RELEASE_MARKER: &[u8] = b"Linux version ";
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

/// The kernel release a shipped kernel image was built as. The image is read
/// as shipped (gunzipped first when it is gzip), so the release is the one
/// the image carries, not the one of any build directory.
pub(super) fn kernel_release(image: &Path) -> Result<String, String> {
    let bytes = std::fs::read(image).map_err(|error| format!("cannot read: {error}"))?;
    let bytes = if bytes.starts_with(GZIP_MAGIC) {
        decompress("gzip", image)?
    } else {
        bytes
    };
    release_in(&bytes).ok_or_else(|| {
        format!(
            "no \"Linux version <release>\" string in {}",
            image.display()
        )
    })
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
