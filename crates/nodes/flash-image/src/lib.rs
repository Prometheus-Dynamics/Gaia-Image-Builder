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
//! Cost: the stream is put together from its blocks (see `blocks.rs`), so
//! only blocks with data are encoded, on up to the allowed cores, and the
//! zero block once. The encoder's memory is about 340 MB plus 224 MB for
//! each block encoded at once (at most the allowed cores, and at most the
//! blocks with data): 815 MB on a 15.6 GB disk image with three data blocks,
//! and about 6 GB for a disk full of data on 26 cores. The step holds that
//! much (rounded up) in the RAM placement's count while it encodes (see
//! `gaia_store::hold_memory`). Its CPU, which runs inside Gaia's process,
//! is counted in the step's report as its own CPU.
//!
//! This is not the general `gaia.compress.xz` node: that one is for any
//! artifact and its presets above 6 use a dictionary a board refuses.

mod blocks;
pub mod dictionary;
pub mod signing;

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Image, ObjectKind};
use std::fs::File;
use std::io::{BufWriter, Write};
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
            let dir = work.dir().to_path_buf();
            let image_stream = dir.join("image.xz");
            let made = encode(&src, &image_stream, threads()).map_err(fail)?;
            work.add_own_cpu(made.worker_cpu_ns);
            work.detail(
                "blocks",
                serde_json::json!({ "blocks": made.blocks, "zero": made.zero_blocks }),
            );
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
                    check_record(&record_stream).map_err(fail)?;
                    concat(&[&image_stream, &record_stream], &out).map_err(fail)?;
                    for scratch in [&image_stream, &record_stream, &record_bin] {
                        std::fs::remove_file(scratch)
                            .map_err(|e| fail(format!("scratch file: {e}")))?;
                    }
                }
            }
            check_streams(&out).map_err(fail)?;
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

/// The multithreaded encoder the image's stream is made with: the step's
/// preset, CRC64, LZMA2 only, blocks of [`BLOCK`] bytes of input, `threads`
/// workers. The block size is fixed, so the bytes are the same at any thread
/// count.
fn mt_encoder(threads: u32, block: u64) -> Result<xz2::stream::Stream, String> {
    xz2::stream::MtStreamBuilder::new()
        .preset(PRESET)
        .check(xz2::stream::Check::Crc64)
        .block_size(block)
        .threads(threads.max(1))
        .encoder()
        .map_err(|e| format!("the xz encoder: {e}"))
}

/// `input` as one xz stream, the bytes `xz -6 --check=crc64
/// --block-size=64MiB -T<threads>` writes, made from its blocks (see
/// [`blocks`]): every block that holds data is encoded, on up to `threads`
/// threads, and the all-zero block once, its bytes written for every block
/// of zeros. A disk image is mostly holes, so most of it is never encoded.
fn encode(input: &Path, out: &Path, threads: u32) -> Result<blocks::Made, String> {
    encode_in(input, out, threads, BLOCK)
}

/// [`encode`] with blocks of `block` bytes: the tests' small blocks make
/// the same kind of stream from megabytes instead of hundreds.
fn encode_in(input: &Path, out: &Path, threads: u32, block: u64) -> Result<blocks::Made, String> {
    blocks::write_stream(input, out, block, threads, &|| mt_encoder(1, block))
}

/// `input` through the multithreaded encoder from start to end: the bytes
/// [`encode`] must write, for the tests to compare.
#[cfg(test)]
fn encode_whole(input: &Path, out: &Path, threads: u32, block: u64) -> Result<(), String> {
    let reader = File::open(input).map_err(|e| format!("encode: {e}"))?;
    let writer = File::create(out).map_err(|e| format!("encode: {e}"))?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, reader);
    let mut encoder = xz2::write::XzEncoder::new_stream(
        BufWriter::with_capacity(1 << 20, writer),
        mt_encoder(threads, block)?,
    );
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

/// The signature record's stream, read back: one block, within the
/// dictionary limit, LZMA2 only (read back refuses any other filter). It is
/// the one-thread encoder's, so its block header has no sizes, and that is
/// not asked of it: the sizes are the image stream's rule.
fn check_record(stream: &Path) -> Result<(), String> {
    let declared = dictionary::read_back_all(stream)?;
    if declared.blocks != 1 {
        return Err(format!(
            "the signature record's stream has {} blocks, not one",
            declared.blocks
        ));
    }
    if declared.dictionary > dictionary::LIMIT {
        return Err(format!(
            "the signature record's dictionary is {} MiB, over the limit of {} MiB",
            declared.dictionary.div_ceil(1024 * 1024),
            dictionary::LIMIT / (1024 * 1024)
        ));
    }
    Ok(())
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

/// The file a board flashes holds at most the image's stream and the
/// signature record's, counted from its bytes.
fn check_streams(file: &Path) -> Result<(), String> {
    let streams = dictionary::stream_count(file)?;
    if streams > dictionary::MAX_STREAMS {
        return Err(format!(
            "the file has {streams} xz streams, over the limit of {}: the image's and the signature record's",
            dictionary::MAX_STREAMS
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

    /// The tests' block: 4 MiB, so a stream of several blocks, holes and
    /// zero blocks is megabytes, not hundreds. The block size is the
    /// encoder's parameter, the same code at either size; the agreed sample
    /// keeps the shipped [`BLOCK`].
    const TEST_BLOCK: u64 = 4 * 1024 * 1024;
    use sha2::{Digest, Sha256};

    /// The sample the format was agreed on: 48 MiB of zeros, 64 MiB of one
    /// repeated line, 48 MiB of a SHA-256 chain's digests, each 512 times.
    fn sample() -> Vec<u8> {
        sample_of(1024 * 1024)
    }

    /// [`sample`] in `unit`s instead of MiB: `sample_of(TEST_BLOCK / 64)`
    /// is its shape at the tests' block size.
    fn sample_of(unit: usize) -> Vec<u8> {
        let mut out = vec![0_u8; 48 * unit];
        let line = b"the quick brown fox jumps over the lazy dog 0123456789\n";
        out.extend(line.iter().cycle().take(64 * unit));
        let mut digest = b"seed".to_vec();
        let end = out.len() + 48 * unit;
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
        // One thread against many at this block size is in
        // `the_stream_from_its_blocks_is_the_whole_encoders`, at the tests'.
        for threads in [4, 13] {
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
        assert!(made[0] == made[1], "the same at 4 and 13 threads");
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

    /// A sparse image as a disk is: blocks of zeros, a data block between
    /// holes, and data in the last block, which is shorter than a block.
    fn sparse() -> Vec<u8> {
        let block = TEST_BLOCK as usize;
        const MIB: usize = TEST_BLOCK as usize / 64;
        let mut out = vec![0_u8; block];
        let line = b"partition table, boot files and a root filesystem\n";
        out.extend(line.iter().cycle().take(block));
        out.resize(4 * block, 0);
        let mut digest = b"tail".to_vec();
        while out.len() < 4 * block + MIB + 3 {
            digest = Sha256::digest(&digest).to_vec();
            out.extend_from_slice(&digest);
        }
        out.truncate(4 * block + MIB + 3);
        out
    }

    /// `raw` in `path` with its zero runs of a MiB or more left as holes.
    fn write_with_holes(path: &Path, raw: &[u8]) {
        use std::os::unix::fs::FileExt;
        const MIB: usize = 1024 * 1024;
        let file = File::create(path).expect("create");
        file.set_len(raw.len() as u64).expect("size");
        for (i, chunk) in raw.chunks(MIB).enumerate() {
            if chunk.iter().any(|b| *b != 0) {
                file.write_all_at(chunk, (i * MIB) as u64).expect("write");
            }
        }
        let allocated = std::process::Command::new("du")
            .args(["-B1", "--"])
            .arg(path)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();
        println!("holes image: {} bytes, du {allocated}", raw.len());
    }

    /// Made from its blocks, with the zero block written once for every
    /// block of zeros, the stream is the whole encoder's, byte for byte, at
    /// any thread count, and the xz program reads it back as the image.
    #[test]
    fn the_stream_from_its_blocks_is_the_whole_encoders() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, raw) in [
            ("sparse", sparse()),
            ("holes", sparse()),
            ("data", sample_of(TEST_BLOCK as usize / 64)),
        ] {
            let image = dir.path().join(format!("{name}.img"));
            if name == "holes" {
                // The same bytes with real holes: only the data is written,
                // so the file system reports the zeros as holes.
                write_with_holes(&image, &raw);
            } else {
                std::fs::write(&image, &raw).expect("write");
            }
            let whole = dir.path().join(format!("{name}-whole.xz"));
            encode_whole(&image, &whole, 4, TEST_BLOCK).expect("encodes");
            let whole = std::fs::read(&whole).expect("read");
            for threads in [1, 3] {
                let out = dir.path().join(format!("{name}-{threads}.xz"));
                let made = encode_in(&image, &out, threads, TEST_BLOCK).expect("encodes");
                let from_blocks = std::fs::read(&out).expect("read");
                assert!(
                    from_blocks == whole,
                    "{name} at {threads} threads: {} bytes against {}",
                    from_blocks.len(),
                    whole.len()
                );
                let expect_zero = if name == "data" { 0 } else { 3 };
                assert_eq!(made.zero_blocks, expect_zero, "{name}");
                assert_eq!(
                    made.blocks,
                    (raw.len() as u64).div_ceil(TEST_BLOCK),
                    "{name}"
                );
                // The encoder threads' CPU is counted: every sample has a
                // block to encode.
                assert!(made.worker_cpu_ns > 0, "{name}: encoder CPU not counted");
                check_dictionary(&out).expect("within a board's limits");
            }
            let xz = std::process::Command::new("xz")
                .args(["-dc"])
                .arg(dir.path().join(format!("{name}-3.xz")))
                .output();
            match xz {
                Ok(out) if out.status.success() => {
                    assert!(
                        out.stdout == raw,
                        "{name}: the xz program reads back the image"
                    )
                }
                _ => println!("SKIPPED the xz program's read-back: none ran"),
            }
        }
    }

    /// The image `GAIA_FLASH_IMAGE` names (a real disk image), both ways:
    /// the same bytes, and how long each took.
    #[test]
    #[ignore = "a real disk image: GAIA_FLASH_IMAGE=<path> cargo test -- --ignored"]
    fn a_real_image_is_the_whole_encoders_bytes() {
        let Some(image) = std::env::var_os("GAIA_FLASH_IMAGE") else {
            println!("SKIPPED: GAIA_FLASH_IMAGE is not set");
            return;
        };
        // GAIA_FLASH_MODE: `blocks` or `whole` makes only that one, to time it
        // alone; unset, both are made and compared.
        let mode = std::env::var("GAIA_FLASH_MODE").unwrap_or_default();
        let image = Path::new(&image);
        let dir = tempfile::tempdir().expect("tempdir");
        let threads = threads();
        let out = dir.path().join("blocks.xz");
        let whole = dir.path().join("whole.xz");
        if mode != "whole" {
            let started = std::time::Instant::now();
            let made = encode(image, &out, threads).expect("encodes");
            println!(
                "from blocks: {:.1} s, {made:?}, {threads} threads",
                started.elapsed().as_secs_f64()
            );
            let bytes = std::fs::read(&out).expect("read");
            println!(
                "from blocks: {} bytes, sha256 {}",
                bytes.len(),
                hex_of(&bytes)
            );
        }
        if mode != "blocks" {
            let started = std::time::Instant::now();
            encode_whole(image, &whole, threads, BLOCK).expect("encodes");
            println!(
                "whole: {:.1} s, {threads} threads",
                started.elapsed().as_secs_f64()
            );
            let bytes = std::fs::read(&whole).expect("read");
            println!("whole: {} bytes, sha256 {}", bytes.len(), hex_of(&bytes));
        }
        if mode.is_empty() {
            let (a, b) = (
                std::fs::read(&out).expect("read"),
                std::fs::read(&whole).expect("read"),
            );
            assert!(a == b, "{} bytes against {}", a.len(), b.len());
            println!("the same bytes");
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

    /// Streams are counted from the bytes: one, two (with stream padding
    /// between them) and three, the last refused by the flash step's check;
    /// bytes that are not whole streams are refused.
    #[test]
    fn the_streams_in_a_file_are_counted_and_a_third_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = dir.path().join("small.img");
        std::fs::write(&image, vec![7_u8; 100_000]).expect("write");
        let one = dir.path().join("one.xz");
        encode(&image, &one, 2).expect("encodes");
        let good = std::fs::read(&one).expect("read");
        assert_eq!(dictionary::stream_count(&one), Ok(1));
        check_streams(&one).expect("one stream");
        let file = dir.path().join("many.xz");
        std::fs::write(&file, [&good[..], &[0; 8], &good[..]].concat()).expect("write");
        assert_eq!(dictionary::stream_count(&file), Ok(2));
        check_streams(&file).expect("two streams");
        std::fs::write(&file, [&good[..], &good[..], &good[..]].concat()).expect("write");
        assert_eq!(dictionary::stream_count(&file), Ok(3));
        let refused = check_streams(&file).expect_err("three streams");
        assert!(refused.contains("3 xz streams"), "{refused}");
        // Junk before the first stream.
        std::fs::write(&file, [&[1_u8; 12][..], &good[..]].concat()).expect("write");
        assert!(dictionary::stream_count(&file).is_err());
        // A cut stream.
        std::fs::write(&file, &good[4..]).expect("write");
        assert!(dictionary::stream_count(&file).is_err());
    }
}
