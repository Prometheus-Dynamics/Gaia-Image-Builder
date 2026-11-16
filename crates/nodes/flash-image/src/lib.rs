//! `gaia.flash.image`: the first-flash file a board accepts.
//!
//! The file is a raw disk image as one xz stream, made with the equivalent
//! of `xz -6 --check=crc64 --block-size=64MiB -T<n>`: liblzma's
//! multithreaded encoder, preset 6, CRC64, LZMA2 only, no BCJ or delta
//! filter, one block for each 64 MiB of image with its sizes in its header.
//! The block size is fixed, so the bytes do not depend on how many threads
//! made them. Preset 6 declares an 8 MiB dictionary, and that is the most a
//! board takes: its decoder memory is fixed at dictionary plus 256 KiB on
//! 64 MB-class boards. A board also takes at most 8192 blocks in the stream.
//! The limits are fixed here and checked after writing: the node reads every
//! block's header back from the bytes on disk, through the stream's index,
//! and fails the step for a dictionary over 8 MiB, a filter other than
//! LZMA2, a check other than CRC64 or more than 8192 blocks (see
//! [`dictionary`]).
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
/// The bytes of image in one block: 64 MiB. A board takes at most
/// [`dictionary::MAX_BLOCKS`] blocks, so an image of up to 512 GiB.
const BLOCK: u64 = 64 * 1024 * 1024;
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
        .config("encoder", "liblzma multithreaded encoder, 64 MiB blocks")
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
            encode(&src, &image_stream, threads()).map_err(fail)?;
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
                    encode_record(&record_bin, &record_stream).map_err(fail)?;
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

/// `input` as one xz stream at the step's preset, CRC64, LZMA2 only, in
/// blocks of [`BLOCK`] bytes of input: liblzma's multithreaded encoder, the
/// one `xz -6 --check=crc64 --block-size=64MiB -T<threads>` runs. The block
/// size is fixed, so the bytes are the same at any thread count.
fn encode(input: &Path, out: &Path, threads: u32) -> Result<(), String> {
    let stream = xz2::stream::MtStreamBuilder::new()
        .preset(PRESET)
        .check(xz2::stream::Check::Crc64)
        .block_size(BLOCK)
        .threads(threads.max(1))
        .encoder()
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

/// The signature record as its own xz stream: one block from liblzma's easy
/// encoder, as before the image's stream had several. Its bytes are part of
/// what a board reads, and are unchanged.
fn encode_record(input: &Path, out: &Path) -> Result<(), String> {
    let stream = xz2::stream::Stream::new_easy_encoder(PRESET, xz2::stream::Check::Crc64)
        .map_err(|e| format!("the xz encoder: {e}"))?;
    let record = std::fs::read(input).map_err(|e| format!("encode: {e}"))?;
    let writer = File::create(out).map_err(|e| format!("encode: {e}"))?;
    let mut encoder = xz2::write::XzEncoder::new_stream(BufWriter::new(writer), stream);
    encoder
        .write_all(&record)
        .map_err(|e| format!("encode: {e}"))?;
    let mut writer = encoder.finish().map_err(|e| format!("encode: {e}"))?;
    writer.flush().map_err(|e| format!("encode: {e}"))
}

/// What `stream` declares, read back from its bytes: fails for a block over
/// the dictionary limit or with a filter other than LZMA2, for more blocks
/// than a board takes, and for a check other than CRC64.
fn check_dictionary(stream: &Path) -> Result<(), String> {
    let declared = dictionary::read_back_all(stream)?;
    check_declared(declared)
}

/// The limits a board holds the image's stream to, on what it declares.
fn check_declared(declared: dictionary::Declared) -> Result<(), String> {
    if declared.dictionary > dictionary::LIMIT {
        return Err(format!(
            "dictionary is {} MiB, over the limit of {} MiB",
            declared.dictionary.div_ceil(1024 * 1024),
            dictionary::LIMIT / (1024 * 1024)
        ));
    }
    if !declared.sized {
        return Err(
            "a block header without its sizes: the stream was not made by the multithreaded encoder, and its bytes are not the ones every build makes"
                .into(),
        );
    }
    if declared.blocks > dictionary::MAX_BLOCKS {
        return Err(format!(
            "the stream has {} blocks, over the limit of {}: the image is larger than {} GiB",
            declared.blocks,
            dictionary::MAX_BLOCKS,
            dictionary::MAX_BLOCKS * BLOCK / (1 << 30)
        ));
    }
    Ok(())
}

/// The CPUs this process may run on: the affinity mask, so a `taskset` cap
/// holds, and 1 when that is not known.
fn threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The sample the format was agreed on: 48 MiB of zeros, 64 MiB of one
    /// repeated line, 48 MiB of a SHA-256 chain's digests, each 512 times.
    fn sample() -> Vec<u8> {
        const MIB: usize = 1024 * 1024;
        let mut out = vec![0_u8; 48 * MIB];
        let line = b"the quick brown fox jumps over the lazy dog 0123456789\n";
        out.extend(line.iter().cycle().take(64 * MIB));
        let mut digest = b"seed".to_vec();
        let end = out.len() + 48 * MIB;
        while out.len() < end {
            digest = Sha256::digest(&digest).to_vec();
            for _ in 0..512 {
                out.extend_from_slice(&digest);
            }
        }
        out.truncate(end);
        out
    }

    fn hex_of(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// The agreed sample: three blocks, the same bytes at any thread count,
    /// the bytes the xz program writes with the agreed command line, and
    /// what a reader gets back is the image.
    #[test]
    fn the_stream_is_the_agreed_one_at_any_thread_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = dir.path().join("sample.img");
        let raw = sample();
        assert_eq!(raw.len(), 160 * 1024 * 1024);
        assert_eq!(
            hex_of(&raw),
            "326c678268e0a2ca05ce03d3c5cc2b0737f6137bb484a53017a998cfd507a16f",
            "the sample's own bytes"
        );
        std::fs::write(&image, &raw).expect("write");
        let mut made = Vec::new();
        for threads in [1, 4, 13] {
            let out = dir.path().join(format!("t{threads}.xz"));
            encode(&image, &out, threads).expect("encodes");
            made.push(std::fs::read(&out).expect("read"));
            let declared = dictionary::read_back_all(&out).expect("reads back");
            assert_eq!(
                declared,
                dictionary::Declared {
                    blocks: 3,
                    dictionary: dictionary::LIMIT,
                    sized: true
                }
            );
            check_dictionary(&out).expect("within a board's limits");
        }
        assert!(
            made[0] == made[1] && made[1] == made[2],
            "the same at 1, 4 and 13 threads"
        );
        println!(
            "builder stream: {} bytes, sha256 {}",
            made[0].len(),
            hex_of(&made[0])
        );
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut xz2::read::XzDecoder::new(&made[0][..]), &mut back)
            .expect("decodes");
        assert!(back == raw, "decodes to the image");

        // The xz program with the agreed command line, when the machine has it.
        let xz = std::process::Command::new("xz")
            .args(["-6", "--check=crc64", "--block-size=64MiB", "-T4", "-c"])
            .arg(&image)
            .output();
        match xz {
            Ok(out) if out.status.success() => {
                println!(
                    "xz program:     {} bytes, sha256 {}",
                    out.stdout.len(),
                    hex_of(&out.stdout)
                );
                let mut theirs = Vec::new();
                std::io::Read::read_to_end(
                    &mut xz2::read::XzDecoder::new(&out.stdout[..]),
                    &mut theirs,
                )
                .expect("decodes");
                assert!(
                    theirs == raw,
                    "the xz program's stream decodes to the image"
                );
            }
            _ => println!("SKIPPED the comparison with the xz program: none ran"),
        }
    }

    #[test]
    fn a_stream_over_a_boards_limits_is_refused() {
        let within = dictionary::Declared {
            blocks: dictionary::MAX_BLOCKS,
            dictionary: dictionary::LIMIT,
            sized: true,
        };
        check_declared(within).expect("at the limits");
        let why = check_declared(dictionary::Declared {
            blocks: dictionary::MAX_BLOCKS + 1,
            ..within
        })
        .expect_err("one block too many");
        assert!(
            why.contains("8193 blocks") && why.contains("512 GiB"),
            "{why}"
        );
        let why = check_declared(dictionary::Declared {
            dictionary: dictionary::LIMIT + 1,
            ..within
        })
        .expect_err("a dictionary too large");
        assert!(why.contains("over the limit of 8 MiB"), "{why}");
        let why = check_declared(dictionary::Declared {
            sized: false,
            ..within
        })
        .expect_err("no sizes in a block header");
        assert!(why.contains("without its sizes"), "{why}");
    }

    /// The one-thread encoder's stream (one block, no sizes in its header)
    /// reads back as not sized, and is not what the step ships.
    #[test]
    fn the_one_thread_encoders_stream_is_told_apart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = dir.path().join("small.img");
        std::fs::write(&image, vec![7_u8; 100_000]).expect("write");
        let single = dir.path().join("single.xz");
        encode_record(&image, &single).expect("encodes");
        let declared = dictionary::read_back_all(&single).expect("reads back");
        assert!(!declared.sized && declared.blocks == 1, "{declared:?}");
        assert!(check_dictionary(&single).is_err());
        let threaded = dir.path().join("threaded.xz");
        encode(&image, &threaded, 1).expect("encodes");
        let declared = dictionary::read_back_all(&threaded).expect("reads back");
        assert!(declared.sized && declared.blocks == 1, "{declared:?}");
        check_dictionary(&threaded).expect("as shipped");
    }

    /// A file that is not one whole stream, or whose index does not account
    /// for it, is refused by the read-back.
    #[test]
    fn the_read_back_refuses_what_is_not_one_whole_stream() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = dir.path().join("small.img");
        std::fs::write(&image, vec![7_u8; 100_000]).expect("write");
        let out = dir.path().join("small.xz");
        encode(&image, &out, 2).expect("encodes");
        let good = std::fs::read(&out).expect("read");
        assert_eq!(dictionary::read_back_all(&out).expect("reads").blocks, 1);
        let bad = dir.path().join("bad.xz");
        // Cut short: no footer where one must be.
        std::fs::write(&bad, &good[..good.len() - 4]).expect("write");
        assert!(dictionary::read_back_all(&bad).is_err());
        // Two streams, one after another: the index found is the second's,
        // and it does not account for the file.
        std::fs::write(&bad, [&good[..], &good[..]].concat()).expect("write");
        assert!(dictionary::read_back_all(&bad).is_err());
        // Not xz at all.
        std::fs::write(&bad, vec![0_u8; 64]).expect("write");
        assert!(dictionary::read_back_all(&bad).is_err());
    }
}
