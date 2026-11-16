//! `gaia.firmware.k230`: one file of the OS's bootloader output, made into the
//! image the K230 ROM, SPL or U-Boot reads, as a tree holding that file.
//!
//! The kinds (the names a board's `wrap` key takes, in describe):
//! - `k230-first-stage`: the 4-byte zero version word and the file, under the
//!   K230 header (the SPL, read by the boot ROM);
//! - `k230-u-boot`: the file is U-Boot's `u-boot.bin`; it is priv gzip, then a
//!   legacy uImage (firmware, gzip, load 0, entry 0, name "uboot", time 0), then
//!   the version word and the K230 header;
//! - `opensbi-uimage`: the file is OpenSBI's `fw_jump.bin`; a legacy uImage only
//!   (kernel, no compression, load 0, entry 0, name "linux", time 0), with no K230
//!   header: U-Boot's `bootm` reads it.
//!
//! The output is checked before it is written: the header's digest, the uImage's
//! header and data CRCs. A file that is already wrapped is refused.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, StoreRef};
use std::path::Path;

const NAME: &str = "gaia.firmware.k230";
/// The recipe version of this node's output: bump it when the bytes change.
const RECIPE: u32 = 1;

/// The kinds a board's `wrap` key names. The strings are the board's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapKind {
    FirstStage,
    UBoot,
    OpensbiUimage,
}

impl WrapKind {
    /// The kind a board's `wrap` key names, or the error that names it.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "k230-first-stage" => Ok(Self::FirstStage),
            "k230-u-boot" => Ok(Self::UBoot),
            "opensbi-uimage" => Ok(Self::OpensbiUimage),
            other => Err(format!(
                "wrap '{other}' is not a kind: use k230-first-stage, k230-u-boot or opensbi-uimage"
            )),
        }
    }

    /// The name a board's `wrap` key gives this kind.
    pub fn name(self) -> &'static str {
        match self {
            Self::FirstStage => "k230-first-stage",
            Self::UBoot => "k230-u-boot",
            Self::OpensbiUimage => "opensbi-uimage",
        }
    }
}

/// The wrapped bytes of `raw` for `kind`, checked by the same reader the ROM,
/// SPL or U-Boot uses.
pub fn wrap_bytes(kind: WrapKind, raw: &[u8]) -> Result<Vec<u8>, String> {
    let fail = |e: crate::Error| e.to_string();
    let out = match kind {
        WrapKind::FirstStage => {
            if raw.starts_with(crate::MAGIC) {
                return Err("the file is already a K230 first-stage image".into());
            }
            crate::wrap_firstage(raw)
        }
        WrapKind::UBoot => {
            if raw.starts_with(crate::MAGIC) {
                return Err("the file is already a K230 first-stage image".into());
            }
            let uimage = crate::uboot_uimage(raw, 0).map_err(fail)?;
            crate::wrap_firstage(&uimage)
        }
        WrapKind::OpensbiUimage => {
            if crate::check_uimage(raw).is_ok() {
                return Err("the file is already a uImage".into());
            }
            crate::opensbi_uimage(raw, 0).map_err(fail)?
        }
    };
    verify(kind, &out)?;
    Ok(out)
}

/// The check half: reads `out` back as its kind's reader does.
pub fn verify(kind: WrapKind, out: &[u8]) -> Result<(), String> {
    match kind {
        WrapKind::FirstStage => {
            crate::parse_firstage(out).map_err(|e| format!("self-check: {e}"))?;
        }
        WrapKind::UBoot => {
            let parsed = crate::parse_firstage(out).map_err(|e| format!("self-check: {e}"))?;
            if parsed.payload.len() < crate::VERSION_LEN {
                return Err("self-check: the payload has no version word".into());
            }
            crate::check_uimage(&parsed.payload[crate::VERSION_LEN..])
                .map_err(|e| format!("self-check: {e}"))?;
        }
        WrapKind::OpensbiUimage => {
            crate::check_uimage(out).map_err(|e| format!("self-check: {e}"))?;
        }
    }
    Ok(())
}

/// The node: the file `path` of the tree `from` (the bootloader output), wrapped
/// as `kind` says, as a tree holding that file under the same name.
#[node(id = "gaia.firmware.k230", inputs(store, from, path, kind), outputs(wrapped))]
pub fn firmware_k230(
    store: &str,
    from: &Artifact,
    path: &str,
    kind: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let fail = |why: String| StepError::body(NAME, format!("'{path}' as {kind}: {why}"));
    let wrap = WrapKind::parse(kind).map_err(fail)?;
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    if from.object.kind != gaia_types::ObjectKind::Tree {
        return Err(fail("its source is a file, not a tree to pick a file from".into()));
    }
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| fail("its source is not in the store".into()))?;
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !plain {
        return Err(fail("not a path inside the tree".into()));
    }
    let file = store.path(&object).join(relative);
    let raw = std::fs::read(&file).map_err(|e| fail(format!("read: {e}")))?;
    let key = KeyBuilder::new(NAME, RECIPE)
        .config("path", path)
        .config("kind", wrap.name())
        .input("from", object.digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let bytes = wrap_bytes(wrap, &raw).map_err(|why| fail(why))?;
            let dir = work.dir().join("tree");
            std::fs::create_dir(&dir).map_err(|e| StepError::body(NAME, e))?;
            std::fs::write(dir.join(path), bytes).map_err(|e| fail(format!("write: {e}")))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("firmware"),
        object: StoreRef::from(made.outputs["tree"]),
    })
}

/// The registry's record for the node.
pub const NODES: [&NodeRecord; 1] = [&FIRMWARE_K230_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_and_name_themselves() {
        for kind in [WrapKind::FirstStage, WrapKind::UBoot, WrapKind::OpensbiUimage] {
            assert_eq!(WrapKind::parse(kind.name()), Ok(kind));
        }
        let why = WrapKind::parse("k230-spl").expect_err("unknown");
        assert!(why.contains("k230-spl"), "{why}");
    }

    #[test]
    fn first_stage_is_checked_and_refuses_pre_wrapped() {
        let out = wrap_bytes(WrapKind::FirstStage, b"spl code").expect("wraps");
        verify(WrapKind::FirstStage, &out).expect("verifies");
        assert!(wrap_bytes(WrapKind::FirstStage, &out).is_err());
    }

    #[test]
    fn u_boot_is_a_uimage_inside_the_first_stage() {
        let raw: Vec<u8> = (0..5000u32).map(|i| (i % 233) as u8).collect();
        let out = wrap_bytes(WrapKind::UBoot, &raw).expect("wraps");
        verify(WrapKind::UBoot, &out).expect("verifies");
    }

    #[test]
    fn opensbi_is_a_uimage_with_no_first_stage_header() {
        let raw = b"fw_jump payload".to_vec();
        let out = wrap_bytes(WrapKind::OpensbiUimage, &raw).expect("wraps");
        assert_ne!(&out[0..4], crate::MAGIC, "no K230 header");
        verify(WrapKind::OpensbiUimage, &out).expect("verifies");
        assert!(wrap_bytes(WrapKind::OpensbiUimage, &out).is_err());
    }

    #[test]
    fn the_check_refuses_a_damaged_output() {
        let mut out = wrap_bytes(WrapKind::OpensbiUimage, b"abc").expect("wraps");
        let last = out.len() - 1;
        out[last] ^= 1;
        assert!(verify(WrapKind::OpensbiUimage, &out).is_err());
    }
}
