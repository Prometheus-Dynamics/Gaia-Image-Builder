//! `gaia.flash.image`: the first-flash file a board accepts.
//!
//! The file is a raw disk image as one xz stream, made with the equivalent
//! of `xz -6 --check=crc64 -T1`: the single-threaded liblzma encoder, preset
//! 6, CRC64, LZMA2 only, no BCJ or delta filter. Preset 6 declares an 8 MiB
//! dictionary, and that is the most a board takes. A board refuses a larger
//! dictionary, since its decoder memory is fixed at dictionary plus 256 KiB
//! on 64 MB-class boards, so the limits are fixed here and checked after
//! writing: the node reads the dictionary back from the bytes on disk and
//! fails the step if it is over 8 MiB or if any filter other than LZMA2 is
//! present (see [`dictionary`]).
//!
//! The file is one xz stream when unsigned, and exactly two when signed:
//! stream 1 followed by stream 2, with no padding between them. Stream 2
//! decodes to exactly one 108-byte record: the magic `LMNSSIG1`, the
//! algorithm (Ed25519), the key id (SHA-256 of the public key) and the
//! signature over the message the board rebuilds from the board id, the
//! format, the raw image's size and SHA-256 (see [`signing`]). The record is last, so `xz -dc --single-stream` gives the
//! image alone.
//!
//! The secret key comes only from the environment: `key_env` names the
//! variable, which holds a minisign secret key file's path or its text, as
//! `gaia.sign` reads it. It is never in the repository or a recipe constant,
//! and it never reaches the store, the step key, a log or an error. The
//! step key holds the image's digest, the board, the format, the encoder
//! settings, the xz library versions, whether the file is signed and, when
//! it is, the public key.
//!
//! This is not the general `gaia.compress.xz` node: that one is for any
//! artifact and its presets above 6 use a dictionary a board refuses.

pub mod dictionary;
pub mod signing;

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Image, ObjectKind};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use zeroize::Zeroizing;

const NAME: &str = "gaia.flash.image";
/// The encoder's preset: 6, which declares an 8 MiB dictionary.
const PRESET: u32 = 6;
/// The file the step makes.
const OUTPUT: &str = "first-flash.img.xz";

/// The libraries this node links, pinned in its Cargo.toml: the xz encoder
/// and the libraries the signature is made with. Its step key names them,
/// and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[
    ("xz2", "0.1.7"),
    ("lzma-sys", "0.1.20"),
    ("ed25519-dalek", "2.2.0"),
    ("sha2", "0.11.1"),
];

/// The first-flash `.img.xz` of the raw `image`, for the board `board` and the
/// image kind `format` (`disk` for a whole-disk image). `key_env` names the
/// environment variable that holds the Ed25519 secret key; unset or empty, the
/// file is unsigned.
#[node(
    id = "gaia.flash.image",
    inputs(store, image, board, format, key_env),
    outputs(file)
)]
pub fn flash_image(
    store: &str,
    image: Image,
    board: &str,
    format: &str,
    key_env: Option<&String>,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    flash_image_with(
        store,
        image,
        board,
        format,
        key_env.map(String::as_str),
        &|name| std::env::var(name).ok(),
        cx,
    )
}

/// [`flash_image`], with the environment passed in: `env` gives a variable's
/// value, or `None` when it is not set. Tests use it to name a key without
/// setting a variable of the process.
pub fn flash_image_with(
    store: &str,
    image: Image,
    board: &str,
    format: &str,
    key_env: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    if board.trim().is_empty() {
        return Err(fail(
            "an empty board id; it names the board the image is for".into(),
        ));
    }
    if format.trim().is_empty() {
        return Err(fail(
            "an empty format; it names the raw image's kind".into(),
        ));
    }
    if image.object.kind != ObjectKind::File {
        return Err(fail("an image that is not a file in the store".into()));
    }
    let object = image
        .object
        .object_ref()
        .ok_or_else(|| fail("an image that is not a file in the store".into()))?;
    let src = store.path(&object);
    if !src.is_file() {
        return Err(fail("an image that is not a file in the store".into()));
    }
    let raw_size = std::fs::metadata(&src)
        .map_err(|e| fail(format!("the image: {e}")))?
        .len();
    if raw_size == 0 {
        return Err(fail("an empty image; there is no data to flash".into()));
    }
    let signer = match key_env.filter(|name| !name.is_empty()) {
        None => None,
        Some(name) => {
            let value = env(name)
                .map(Zeroizing::new)
                .ok_or_else(|| fail(format!("{name} is not set; it names the signing key")))?;
            Some(load_key(name, value)?)
        }
    };
    let public = signer.as_ref().map(|key| key.verifying_key());
    let mut key = KeyBuilder::new(NAME, 1)
        .config("board", board)
        .config("format", format)
        .config("encoder", "liblzma easy encoder, one thread")
        .config("preset", PRESET.to_string())
        .config("check", "crc64")
        .config("dictionary limit", dictionary::LIMIT.to_string())
        .config("signed", if public.is_some() { "yes" } else { "no" })
        .input("image", object.digest)
        .libs(LIBS);
    if let Some(public) = &public {
        key = key.config("public key", hex(public.as_bytes()));
    }
    let key = key.finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("file", Path::new(OUTPUT))],
        |work| {
            let dir = work.dir();
            let image_stream = dir.join("image.xz");
            encode(&src, &image_stream).map_err(fail)?;
            check_dictionary(&image_stream).map_err(fail)?;
            let out = dir.join(OUTPUT);
            match &signer {
                None => std::fs::rename(&image_stream, &out)
                    .map_err(|e| fail(format!("the image: {e}")))?,
                Some(signer) => {
                    let message =
                        signing::message(board, format, raw_size, object.digest.as_bytes())
                            .map_err(fail)?;
                    let signature = ed25519_sign(signer, &message);
                    let public = signer.verifying_key();
                    let record = signing::record(&signing::key_id(public.as_bytes()), &signature);
                    let record_bin = dir.join("record.bin");
                    std::fs::write(&record_bin, record)
                        .map_err(|e| fail(format!("the record: {e}")))?;
                    let record_stream = dir.join("record.xz");
                    encode(&record_bin, &record_stream).map_err(fail)?;
                    check_dictionary(&record_stream).map_err(fail)?;
                    concat(&[&image_stream, &record_stream], &out).map_err(fail)?;
                    for scratch in [&image_stream, &record_stream, &record_bin] {
                        std::fs::remove_file(scratch)
                            .map_err(|e| fail(format!("scratch file: {e}")))?;
                    }
                }
            }
            Ok(())
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse(&format!("{format}+xz")),
        object: made.outputs["file"].into(),
    })
}

/// The secret key the variable `name` holds, a minisign secret key file's
/// text or the path of one. Errors name the variable, never its value.
fn load_key(name: &str, value: Zeroizing<String>) -> Result<ed25519_dalek::SigningKey, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let text = if value.trim_start().starts_with("untrusted comment:") {
        value
    } else {
        Zeroizing::new(
            std::fs::read_to_string(value.trim())
                .map_err(|e| fail(format!("the key file {name} names: {}", e.kind())))?,
        )
    };
    signing::secret_key(&text).map_err(|e| fail(format!("{name}: {e}")))
}

/// The Ed25519 signature of `message` (not a prehash).
fn ed25519_sign(key: &ed25519_dalek::SigningKey, message: &[u8]) -> [u8; 64] {
    use ed25519_dalek::Signer;
    key.sign(message).to_bytes()
}

/// `input` as one xz stream at the step's preset, CRC64, LZMA2 only, on one
/// thread: liblzma's easy encoder, the encoder `xz -T1` runs.
fn encode(input: &Path, out: &Path) -> Result<(), String> {
    let stream = xz2::stream::Stream::new_easy_encoder(PRESET, xz2::stream::Check::Crc64)
        .map_err(|e| format!("the xz encoder: {e}"))?;
    let reader = File::open(input).map_err(|e| format!("encode: {e}"))?;
    let writer = File::create(out).map_err(|e| format!("encode: {e}"))?;
    let mut reader = BufReader::with_capacity(1 << 20, reader);
    let mut encoder =
        xz2::write::XzEncoder::new_stream(BufWriter::with_capacity(1 << 20, writer), stream);
    std::io::copy(&mut reader, &mut encoder).map_err(|e| format!("encode: {e}"))?;
    let mut writer = encoder.finish().map_err(|e| format!("encode: {e}"))?;
    writer.flush().map_err(|e| format!("encode: {e}"))
}

/// The dictionary `stream` declares, read back from its bytes: fails over
/// the limit, and for a block with a filter other than LZMA2.
fn check_dictionary(stream: &Path) -> Result<(), String> {
    let size = dictionary::read_back(stream)?;
    if size > dictionary::LIMIT {
        return Err(format!(
            "dictionary is {} MiB, over the limit of {} MiB",
            size.div_ceil(1024 * 1024),
            dictionary::LIMIT / (1024 * 1024)
        ));
    }
    Ok(())
}

/// The parts, one after another, in `out`.
fn concat(parts: &[&Path], out: &Path) -> Result<(), String> {
    let mut writer = BufWriter::with_capacity(
        1 << 20,
        File::create(out).map_err(|e| format!("concatenate: {e}"))?,
    );
    for part in parts {
        let mut reader = File::open(part).map_err(|e| format!("concatenate: {e}"))?;
        std::io::copy(&mut reader, &mut writer).map_err(|e| format!("concatenate: {e}"))?;
    }
    writer.flush().map_err(|e| format!("concatenate: {e}"))
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FLASH_IMAGE_NODE];
