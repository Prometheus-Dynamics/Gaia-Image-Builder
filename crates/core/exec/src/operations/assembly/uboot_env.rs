//! The `uboot-env` transform: a U-Boot environment image, the bytes
//! `mkenvimage` writes and U-Boot's `env_t` reads. The image is
//! `CONFIG_ENV_SIZE` bytes: a CRC32 of the data area (little-endian, as on
//! the ARM and RISC-V boards it is for), a flags byte when the environment
//! is redundant, then the data area: each `name=value` and a NUL, one more
//! NUL after the last, and 0xff to the end.

use super::*;
use std::collections::BTreeSet;

/// The flags byte of a redundant environment written by a build: the
/// active copy (`ENV_REDUND_ACTIVE`).
const REDUNDANT_ACTIVE: u8 = 1;
/// `mkenvimage`'s fill byte for the space after the variables.
const PAD: u8 = 0xff;

/// The `name=value` lines of `text`, in order: blank lines and `#` comments
/// are skipped, and each other line must have a name before its `=`.
fn variables(text: &str) -> Result<Vec<(&str, &str)>, String> {
    let mut seen = BTreeSet::new();
    let mut variables = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
            .ok_or_else(|| format!("line {} is not 'name=value': '{line}'", index + 1))?;
        if !seen.insert(name) {
            return Err(format!("line {}: '{name}' is set twice", index + 1));
        }
        variables.push((name, value));
    }
    Ok(variables)
}

/// The environment image of `text`, `size` bytes long.
pub(super) fn uboot_env_image(text: &str, size: u64, redundant: bool) -> Result<Vec<u8>, String> {
    let header = if redundant { 5 } else { 4 };
    let mut data = Vec::new();
    for (name, value) in variables(text)? {
        data.extend_from_slice(name.as_bytes());
        data.push(b'=');
        data.extend_from_slice(value.as_bytes());
        data.push(0);
    }
    data.push(0);
    let capacity = (size as usize).saturating_sub(header);
    if data.len() > capacity {
        return Err(format!(
            "the variables take {} bytes, more than the {capacity} an environment of {size} bytes holds",
            data.len()
        ));
    }
    data.resize(capacity, PAD);
    let mut image = crc32(&data).to_le_bytes().to_vec();
    if redundant {
        image.push(REDUNDANT_ACTIVE);
    }
    image.extend_from_slice(&data);
    Ok(image)
}

pub(super) fn write_uboot_env(
    transform: &gaia_spec::AssemblyTransformSpec,
    src: PathBuf,
    dest: PathBuf,
) -> Result<AssemblyTransformSummary, AssemblyError> {
    let size = transform
        .env_size
        .as_deref()
        .ok_or("a uboot-env transform needs env_size")?
        .parse::<gaia_spec::ByteSize>()
        .map_err(|error| format!("uboot-env env_size: {error}"))?
        .bytes();
    let text = std_fs::read_to_string(&src)
        .map_err(|error| format!("failed to read '{}': {error}", src.display()))?;
    let image = uboot_env_image(&text, size, transform.redundant)
        .map_err(|error| format!("uboot-env '{}': {error}", src.display()))?;
    let temp = temporary_assembly_output_path(&dest);
    std_fs::write(&temp, &image)
        .map_err(|error| format!("failed to write '{}': {error}", temp.display()))?;
    let sha256 = publish_assembly_output(&temp, &dest)?;
    Ok(AssemblyTransformSummary {
        src,
        bytes: image.len() as u64,
        sha256,
        dest,
        tool_path: None,
        tool_version: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_is_crc_data_and_padding_as_mkenvimage_writes_it() {
        let image = uboot_env_image(
            "# A/B\nboot_slot=a\n\nbootcount=0\nbootlimit=3\naltbootcmd=setenv boot_slot b; run bootcmd\n",
            0x1000,
            false,
        )
        .expect("image");

        assert_eq!(image.len(), 0x1000);
        let data = &image[4..];
        let vars = b"boot_slot=a\0bootcount=0\0bootlimit=3\0altbootcmd=setenv boot_slot b; run bootcmd\0\0";
        assert_eq!(&data[..vars.len()], vars);
        assert!(data[vars.len()..].iter().all(|byte| *byte == 0xff));
        assert_eq!(image[..4], crc32(data).to_le_bytes());
    }

    #[test]
    fn a_redundant_image_has_the_active_flag_after_the_crc() {
        let image = uboot_env_image("a=1\n", 64, true).expect("image");

        assert_eq!(image[4], 1);
        assert_eq!(&image[5..10], b"a=1\0\0");
        assert_eq!(image[..4], crc32(&image[5..]).to_le_bytes());
    }

    #[test]
    fn bad_lines_duplicates_and_overflow_are_errors() {
        assert!(
            uboot_env_image("bootcmd run x\n", 64, false)
                .expect_err("no =")
                .contains("line 1 is not 'name=value'")
        );
        assert!(
            uboot_env_image("a=1\na=2\n", 64, false)
                .expect_err("twice")
                .contains("'a' is set twice")
        );
        assert!(
            uboot_env_image("a=123456789\n", 16, false)
                .expect_err("overflow")
                .contains("more than the 12")
        );
    }
}
