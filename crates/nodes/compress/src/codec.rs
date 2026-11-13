//! `gaia.compress.{xz,zstd,gzip}`: one artifact compressed.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::Digest;
use gaia_types::{Artifact, ArtifactKind};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

#[derive(Clone, Copy)]
enum Format {
    Xz,
    Zstd,
    Gzip,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Self::Xz => "gaia.compress.xz",
            Self::Zstd => "gaia.compress.zstd",
            Self::Gzip => "gaia.compress.gzip",
        }
    }

    fn libs(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Self::Xz => crate::libs::XZ.to_vec(),
            Self::Zstd => crate::libs::libzstd().to_vec(),
            Self::Gzip => crate::libs::GZIP.to_vec(),
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Xz => "xz",
            Self::Zstd => "zstd",
            Self::Gzip => "gz",
        }
    }

    fn levels(self) -> std::ops::RangeInclusive<i64> {
        match self {
            Self::Xz => 0..=9,
            Self::Zstd => 1..=22,
            Self::Gzip => 0..=9,
        }
    }
}

/// `artifact` compressed with xz at `level` (0 to 9) on `threads` threads.
/// This is a general compressor: its presets above 6 use a dictionary above
/// 8 MiB, which a board refuses; the file a board takes is made by
/// `gaia.flash.image`.
#[node(
    id = "gaia.compress.xz",
    inputs(store, artifact, level, threads),
    outputs(compressed)
)]
pub fn compress_xz(
    store: &str,
    artifact: Artifact,
    level: i64,
    threads: i64,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    compress(Format::Xz, store, &artifact, level, threads, cx)
}

/// `artifact` compressed with zstd at `level` (1 to 22) on `threads` threads.
#[node(
    id = "gaia.compress.zstd",
    inputs(store, artifact, level, threads),
    outputs(compressed)
)]
pub fn compress_zstd(
    store: &str,
    artifact: Artifact,
    level: i64,
    threads: i64,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    compress(Format::Zstd, store, &artifact, level, threads, cx)
}

/// `artifact` compressed with gzip at `level` (0 to 9), on one thread.
#[node(
    id = "gaia.compress.gzip",
    inputs(store, artifact, level),
    outputs(compressed)
)]
pub fn compress_gzip(
    store: &str,
    artifact: Artifact,
    level: i64,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    compress(Format::Gzip, store, &artifact, level, 1, cx)
}

fn compress(
    format: Format,
    store: &str,
    artifact: &Artifact,
    level: i64,
    threads: i64,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let name = format.name();
    let store = Store::open(store).map_err(StepError::store(name))?;
    if !format.levels().contains(&level) {
        let range = format.levels();
        return Err(StepError::body(
            name,
            format!(
                "level {level} is outside {}..={}",
                range.start(),
                range.end()
            ),
        ));
    }
    let threads = u32::try_from(threads.max(1)).unwrap_or(u32::MAX);
    let object = artifact
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(name, "the artifact is not in the store"))?;
    let input = store.path(&object);
    // The thread count is not in the key: the output does not depend on it.
    let key = codec_key(name, level, object.digest, &format.libs());
    let made = Step::new(&store, cx, name, key).look_up_or_make(
        &[("compressed", Path::new("compressed"))],
        |work| {
            let out = work.dir().join("compressed");
            encode(format, &input, &out, level as u32, threads)
                .map_err(|e| StepError::body(name, e.to_string()))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse(&format!("{}+{}", artifact.kind, format.suffix())),
        object: made.outputs["compressed"].into(),
    })
}

/// The key of one compression: its name, level, input and the libraries
/// that encode it. The thread count is not in it: the output does not
/// depend on it.
fn codec_key(name: &str, level: i64, input: Digest, libs: &[(&str, &str)]) -> gaia_store::StepKey {
    KeyBuilder::new(name, 1)
        .config("level", level.to_string())
        .input("input", input)
        .libs(libs)
        .finish()
}

fn encode(
    format: Format,
    input: &Path,
    out: &Path,
    level: u32,
    threads: u32,
) -> std::io::Result<()> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(input)?);
    let writer = BufWriter::with_capacity(1 << 20, File::create(out)?);
    match format {
        Format::Xz => {
            // The multithreaded encoder even on one thread: its blocks, set by
            // the preset, are what make the output the same at any count.
            let stream = xz2::stream::MtStreamBuilder::new()
                .preset(level)
                .threads(threads)
                .check(xz2::stream::Check::Crc64)
                .encoder()
                .map_err(std::io::Error::other)?;
            let mut encoder = xz2::write::XzEncoder::new_stream(writer, stream);
            std::io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?.flush()
        }
        Format::Zstd => {
            let mut encoder = zstd::stream::write::Encoder::new(writer, level as i32)?;
            // At least one worker: zstd's jobs, set by the level, make the
            // output the same at any count; zero workers is another format.
            encoder.multithread(threads)?;
            encoder.include_checksum(true)?;
            std::io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?.flush()
        }
        Format::Gzip => {
            // No name and mtime 0 in the header.
            let mut encoder = flate2::GzBuilder::new()
                .mtime(0)
                .write(writer, flate2::Compression::new(level));
            std::io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?.flush()
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::codec_key;
    use gaia_store::Digest;

    #[test]
    fn a_library_version_changes_the_key() {
        let input = Digest::of_bytes(b"an artifact");
        let key = |xz2: &str| {
            codec_key(
                "gaia.compress.xz",
                6,
                input,
                &[("xz2", xz2), ("lzma-sys", "0.1.20")],
            )
        };
        assert_eq!(key("0.1.7"), key("0.1.7"));
        assert_ne!(key("0.1.7"), key("0.1.8"));
    }
}
