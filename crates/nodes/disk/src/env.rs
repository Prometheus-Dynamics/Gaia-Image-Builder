//! The U-Boot environment: the bytes `mkenvimage` writes and U-Boot's
//! `env_t` reads. An environment is `size` bytes: a CRC-32 of the data area
//! (little-endian, as the ARM and RISC-V boards it is for read it), a flags
//! byte when it is redundant, then the data area: each `name=value` and a NUL,
//! one more NUL after the last variable, and 0xff to the end.
//!
//! `copies` writes the environment that many times in a row, for boards
//! that keep two copies in one partition with no flags byte of their own.

use crate::UbootVar;
use crate::crc::crc32;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, StoreRef};
use std::collections::BTreeSet;
use std::path::Path;

const NAME: &str = "gaia.uboot.env";
/// The flags byte of a redundant environment: the active copy.
const REDUNDANT_ACTIVE: u8 = 1;
/// `mkenvimage`'s fill byte for the space after the variables.
const PAD: u8 = 0xff;

/// The environment image of `vars`, `size` bytes long, once.
pub fn uboot_env_image(vars: &[UbootVar], size: u64, redundant: bool) -> Result<Vec<u8>, String> {
    let header: usize = if redundant { 5 } else { 4 };
    let mut seen = BTreeSet::new();
    let mut data = Vec::new();
    for var in vars {
        if var.name.is_empty()
            || var.name.contains('=')
            || var.name.chars().any(char::is_whitespace)
            || var.name.contains('\0')
            || var.value.contains('\0')
        {
            return Err(format!("'{}' is not a variable name", var.name));
        }
        if !seen.insert(var.name.as_str()) {
            return Err(format!("'{}' is set twice", var.name));
        }
        data.extend_from_slice(var.name.as_bytes());
        data.push(b'=');
        data.extend_from_slice(var.value.as_bytes());
        data.push(0);
    }
    data.push(0);
    let size = usize::try_from(size).map_err(|_| "the environment is too large".to_string())?;
    let capacity = size.saturating_sub(header);
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

/// The U-Boot environment node: the variables as an environment of `size`
/// bytes, written `copies` times. The artifact is the partition's content.
#[node(
    id = "gaia.uboot.env",
    inputs(store, id, vars, size, redundant, copies),
    outputs(artifact)
)]
#[allow(clippy::too_many_arguments)]
pub fn uboot_env(
    store: &str,
    id: &str,
    vars: Vec<UbootVar>,
    size: u64,
    redundant: bool,
    copies: u32,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    uboot_env_artifact(store, id, &vars, size, redundant, copies, cx)
}

/// The node's work, as [`uboot_env`] does it.
pub fn uboot_env_artifact(
    store: &str,
    id: &str,
    vars: &[UbootVar],
    size: u64,
    redundant: bool,
    copies: u32,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, format!("uboot env '{id}': {e}"));
    if copies == 0 {
        return Err(fail(
            "copies is 0; at least 1 environment is written".into(),
        ));
    }
    let once = uboot_env_image(vars, size, redundant).map_err(fail)?;
    let mut key = KeyBuilder::new(NAME, 1)
        .config("id", id)
        .config("size", size.to_string())
        .config("redundant", if redundant { "yes" } else { "no" })
        .config("copies", copies.to_string());
    for (index, var) in vars.iter().enumerate() {
        key = key
            .config(&format!("var {index} name"), &var.name)
            .config(&format!("var {index} value"), &var.value);
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("artifact", Path::new("env.bin"))],
        |work| {
            let bytes = once.repeat(copies as usize);
            std::fs::write(work.dir().join("env.bin"), bytes)
                .map_err(|e| fail(format!("write: {e}")))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("uboot-env"),
        object: StoreRef::from(made.outputs["artifact"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, value: &str) -> UbootVar {
        UbootVar {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn the_image_is_crc_data_and_padding() {
        let vars = [
            var("boot_slot", "a"),
            var("bootcount", "0"),
            var("bootlimit", "3"),
            var("altbootcmd", "setenv boot_slot b; run bootcmd"),
        ];
        let image = uboot_env_image(&vars, 0x1000, false).expect("image");
        assert_eq!(image.len(), 0x1000);
        let data = &image[4..];
        let text = b"boot_slot=a\0bootcount=0\0bootlimit=3\0altbootcmd=setenv boot_slot b; run bootcmd\0\0";
        assert_eq!(&data[..text.len()], text);
        assert!(data[text.len()..].iter().all(|byte| *byte == 0xff));
        assert_eq!(image[..4], crc32(data).to_le_bytes());
    }

    #[test]
    fn a_redundant_image_has_the_active_flag_after_the_crc() {
        let image = uboot_env_image(&[var("a", "1")], 64, true).expect("image");
        assert_eq!(image[4], 1);
        assert_eq!(&image[5..10], b"a=1\0\0");
        assert_eq!(image[..4], crc32(&image[5..]).to_le_bytes());
    }

    #[test]
    fn bad_names_duplicates_and_overflow_are_refused() {
        assert!(
            uboot_env_image(&[var("a b", "1")], 64, false)
                .expect_err("space")
                .contains("'a b' is not a variable name")
        );
        assert!(
            uboot_env_image(&[var("a", "1"), var("a", "2")], 64, false)
                .expect_err("twice")
                .contains("'a' is set twice")
        );
        assert!(
            uboot_env_image(&[var("a", "123456789")], 16, false)
                .expect_err("overflow")
                .contains("more than the 12")
        );
    }
}
