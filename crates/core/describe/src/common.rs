//! What the three formats share: sources, file references, paths and sizes.

use crate::Error;
use gaia_lock::Want;
use gaia_types::Size;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// A path written in a description: relative to the file it is written in.
/// Once read it holds that file's directory joined with what was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelPath(pub PathBuf);

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let path = Path::new(&text);
        if text.is_empty() {
            return Err(de::Error::custom("an empty path"));
        }
        if path.is_absolute() {
            return Err(de::Error::custom(format!(
                "`{text}` is absolute; write it relative to this file"
            )));
        }
        Ok(Self(path.to_path_buf()))
    }
}

impl RelPath {
    /// Joins it to `dir`, the directory of the file it is written in, and
    /// drops `.` and `name/..` pairs.
    pub(crate) fn resolve(&mut self, dir: &Path) {
        self.0 = normalize(&dir.join(&self.0));
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A size: a whole number of bytes, or text with a binary unit: `64K`,
/// `16M`, `1G`, `2T` (`KiB` and `MiB` read the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SizeText(pub Size);

impl<'de> Deserialize<'de> for SizeText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bytes(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bytes(bytes) => Ok(Self(Size(bytes))),
            Raw::Text(text) => parse_size(&text)
                .map(|size| Self(Size(size)))
                .ok_or_else(|| {
                    de::Error::custom(format!(
                        "`{text}` is not a size; write bytes, or a number with K, M, G or T"
                    ))
                }),
        }
    }
}

fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let shift = match unit.trim() {
        "" | "B" => 0,
        "K" | "KiB" => 10,
        "M" | "MiB" => 20,
        "G" | "GiB" => 30,
        "T" | "TiB" => 40,
        _ => return None,
    };
    number.checked_mul(1 << shift)
}

/// A file mode, written as octal text: `"0755"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Mode(pub u32);

impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        u32::from_str_radix(&text, 8)
            .ok()
            .filter(|mode| *mode <= 0o7777)
            .map(Self)
            .ok_or_else(|| {
                de::Error::custom(format!("`{text}` is not an octal mode like \"0644\""))
            })
    }
}

/// A directory inside a git source's checkout, written `src/little/uboot`.
/// Only normal names: no leading `/`, no `.` or `..`, not empty. Stored as
/// those names joined by `/`, so `a//b/` and `a/b` are one directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Subdir(String);

impl<'de> Deserialize<'de> for Subdir {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

impl Subdir {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err(
                "an empty `subdir`; write a directory of the checkout, like `src/uboot`".into(),
            );
        }
        let mut names = Vec::new();
        for part in Path::new(text).components() {
            match part {
                Component::Normal(name) => names.push(name.to_string_lossy().into_owned()),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "`{text}` is absolute; a `subdir` is a path inside the checkout"
                    ));
                }
                Component::CurDir | Component::ParentDir => {
                    return Err(format!(
                        "`{text}` has `.` or `..`; a `subdir` is a plain path inside the checkout"
                    ));
                }
            }
        }
        Ok(Self(names.join("/")))
    }

    /// The directory's path inside the checkout, `/`-separated.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a source comes from. The lock pins it: a git source to a commit, a
/// download to its digest, an image to its manifest digest, a directory to
/// its tree digest.
///
/// Written as exactly one of `git` (with `ref`, and an optional `subdir`
/// naming a directory of the checkout to use instead of its root),
/// `download`, `image` (with an optional `platform`), `path` or `rust` (with
/// `targets`). The lock pins
/// the repository's commit; `subdir` is not in the lock. An `image` may name
/// its digest as `reference@sha256:<64 hex>`, which the lock then records
/// without asking the registry.
///
/// A `rust` source is a Rust toolchain release, `rust = "1.99.0"`, with the
/// target triples whose standard library a build needs, `targets =
/// ["aarch64-unknown-linux-musl"]`: the official release files (see
/// [`RustPiece`]), each pinned in the lock by its digest. A Rust build takes
/// its toolchain from the component's source named `toolchain`, never from
/// the build machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SourceText")]
pub enum Source {
    Git {
        url: String,
        reference: String,
        subdir: Option<Subdir>,
    },
    Download {
        url: String,
    },
    Image {
        reference: String,
        platform: ImagePlatform,
    },
    Path {
        path: RelPath,
    },
    Rust {
        version: String,
        /// Sorted, no repeats.
        targets: Vec<String>,
    },
}

/// Where the official Rust releases are.
pub const RUST_DIST: &str = "https://static.rust-lang.org/dist";

/// One file of a Rust toolchain release: what it holds and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustPiece {
    /// The lock name's last part: `rustc`, `cargo` or `std.<triple>`.
    pub suffix: String,
    pub url: String,
}

/// The triple of the machine running the build, as Rust names it: the host
/// rustc, cargo and std are for it.
pub fn rust_host_triple() -> String {
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}

/// The files of Rust `version` for this machine with the std of each of
/// `targets` (and of the host, which build scripts and proc macros need),
/// in the order: rustc, cargo, then the stds by triple.
pub fn rust_pieces(version: &str, targets: &[String]) -> Vec<RustPiece> {
    let host = rust_host_triple();
    let piece = |suffix: String, name: &str, triple: &str| RustPiece {
        suffix,
        url: format!("{RUST_DIST}/{name}-{version}-{triple}.tar.xz"),
    };
    let mut triples: Vec<&str> = targets.iter().map(String::as_str).collect();
    triples.push(&host);
    triples.sort_unstable();
    triples.dedup();
    let mut pieces = vec![
        piece("rustc".into(), "rustc", &host),
        piece("cargo".into(), "cargo", &host),
    ];
    for triple in triples {
        pieces.push(piece(format!("std.{triple}"), "rust-std", triple));
    }
    pieces
}

/// A Rust version as the releases name it: three numbers, `1.99.0`.
fn rust_version(text: &str) -> Result<(), String> {
    let parts: Vec<&str> = text.split('.').collect();
    let numbers = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if numbers {
        Ok(())
    } else {
        Err(format!(
            "`{text}` is not a Rust release: write its three numbers, `1.99.0`"
        ))
    }
}

/// A target triple: lowercase letters, digits, `_` and `.`, in parts
/// joined by `-`.
fn rust_triple(text: &str) -> Result<(), String> {
    let good = text.split('-').count() >= 2
        && text.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
        });
    if good {
        Ok(())
    } else {
        Err(format!("`{text}` is not a target triple"))
    }
}

/// Which machine an image source is pulled for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagePlatform {
    /// The board's target: an image whose files go onto the board.
    #[default]
    Target,
    /// The machine running the build: a root a builder runs its tools in.
    Host,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceText {
    git: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    download: Option<String>,
    #[serde(default, deserialize_with = "image_text")]
    image: Option<String>,
    platform: Option<ImagePlatform>,
    path: Option<RelPath>,
    subdir: Option<Subdir>,
    rust: Option<String>,
    targets: Option<Vec<String>>,
}

/// An image written as `reference`, or `reference@sha256:<64 lowercase hex>`
/// to name its manifest digest. Anything after `@` must be that digest, so
/// a malformed one is refused at `sources.<name>.image`.
fn image_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let Some(text) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if let Some((_, digest)) = text.split_once('@') {
        let hex = digest.strip_prefix("sha256:").unwrap_or_default();
        let valid = hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !valid {
            return Err(de::Error::custom(format!(
                "`{digest}` is not a digest; write `@sha256:` and 64 lowercase hex digits, or leave the `@` part out"
            )));
        }
    }
    Ok(Some(text))
}

impl TryFrom<SourceText> for Source {
    type Error = String;

    fn try_from(text: SourceText) -> Result<Self, String> {
        let has_ref = text.reference.is_some();
        let has_platform = text.platform.is_some();
        let has_subdir = text.subdir.is_some();
        let has_targets = text.targets.is_some();
        if let Some(version) = text.rust {
            if text.git.is_some()
                || text.download.is_some()
                || text.image.is_some()
                || text.path.is_some()
            {
                return Err("only one of `git`, `download`, `image`, `path` and `rust`".into());
            }
            if has_ref || has_platform || has_subdir {
                return Err("a rust source has only `rust` and `targets`".into());
            }
            rust_version(&version)?;
            let mut targets = text.targets.ok_or(
                "a rust source needs `targets`: the target triples its builds are for, `[\"aarch64-unknown-linux-musl\"]`",
            )?;
            for target in &targets {
                rust_triple(target)?;
            }
            targets.sort();
            targets.dedup();
            return Ok(Self::Rust { version, targets });
        }
        if has_targets {
            return Err("only a rust source has `targets`".into());
        }
        let source = match (text.git, text.download, text.image, text.path) {
            (Some(url), None, None, None) => Self::Git {
                url,
                reference: text
                    .reference
                    .ok_or("a git source needs `ref`: the tag or branch")?,
                subdir: text.subdir,
            },
            (None, Some(url), None, None) => Self::Download { url },
            (None, None, Some(reference), None) => Self::Image {
                reference,
                platform: text.platform.unwrap_or_default(),
            },
            (None, None, None, Some(path)) => Self::Path { path },
            (None, None, None, None) => {
                return Err(
                    "say where it comes from: `git`, `download`, `image`, `path` or `rust`".into(),
                );
            }
            _ => return Err("only one of `git`, `download`, `image` and `path`".into()),
        };
        if has_ref && !matches!(source, Self::Git { .. }) {
            return Err("only a git source has a `ref`".into());
        }
        if has_platform && !matches!(source, Self::Image { .. }) {
            return Err("only an image source has a `platform`".into());
        }
        if has_subdir && !matches!(source, Self::Git { .. }) {
            return Err("only a git source has a `subdir`".into());
        }
        Ok(source)
    }
}

impl Source {
    /// What the lock must hold for it, under `name` (`<file's name>.<source>`):
    /// one entry, or for a `rust` source one per release file, named
    /// `<name>.<piece>` (`orion.toolchain.std.aarch64-unknown-linux-musl`).
    pub fn wants(&self, name: &str) -> Vec<(String, Want)> {
        match self {
            Self::Rust { version, targets } => rust_pieces(version, targets)
                .into_iter()
                .map(|piece| {
                    (
                        format!("{name}.{}", piece.suffix),
                        Want::Download { url: piece.url },
                    )
                })
                .collect(),
            other => vec![(name.to_string(), other.want())],
        }
    }

    /// What the lock must hold for a source that is one entry. A `rust`
    /// source is several: see [`Source::wants`].
    fn want(&self) -> Want {
        match self {
            Self::Git { url, reference, .. } => Want::Git {
                url: url.clone(),
                reference: reference.clone(),
            },
            Self::Download { url } => Want::Download { url: url.clone() },
            Self::Image { reference, .. } => Want::Image {
                reference: reference.clone(),
            },
            Self::Path { path } => Want::Tree {
                path: path.0.display().to_string(),
            },
            Self::Rust { .. } => unreachable!("a rust source's wants are its pieces"),
        }
    }

    pub(crate) fn resolve(&mut self, dir: &Path) {
        if let Self::Path { path } = self {
            path.resolve(dir);
        }
    }
}

/// Resolves the paths of a file's `[sources]`.
pub(crate) fn resolve_sources(sources: &mut BTreeMap<String, Source>, dir: &Path) {
    for source in sources.values_mut() {
        source.resolve(dir);
    }
}

/// A file a step takes in: a path inside a source,
/// `{ source = "rkbin", path = "bin/rk35/ddr.bin" }`, a file beside the
/// description, `{ file = "assets/boot.cmd" }`, or a named input the command
/// line gives, `{ input = "bench-key" }` (`gaia plan --input bench-key=PATH`).
/// A component's install entry also takes one of the board's descriptions by
/// name, `{ description = "daemon" }`: the file the image's board states
/// under `[descriptions]`. Only an install entry takes a named input or a
/// description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FileRefText")]
pub enum FileRef {
    InSource {
        source: String,
        path: String,
    },
    Local {
        file: RelPath,
    },
    /// A file the command line names. The name is lowercase letters, digits
    /// and `-` (see [`is_input_name`]), checked where it is used.
    Input {
        name: String,
    },
    /// One of the board's `[descriptions]`, by name.
    Description {
        name: String,
    },
    /// A path in what the component's `[generate]` tool wrote.
    Generated {
        path: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRefText {
    source: Option<String>,
    path: Option<String>,
    file: Option<RelPath>,
    input: Option<String>,
    description: Option<String>,
    generated: Option<String>,
}

impl TryFrom<FileRefText> for FileRef {
    type Error = String;

    fn try_from(text: FileRefText) -> Result<Self, String> {
        if let Some(path) = text.generated {
            return match (
                text.description,
                text.input,
                text.source,
                text.path,
                text.file,
            ) {
                (None, None, None, None, None) => Ok(Self::Generated { path }),
                _ => Err("a generated file is not combined with another kind of source".into()),
            };
        }
        if let Some(name) = text.description {
            return match (text.input, text.source, text.path, text.file) {
                (None, None, None, None) => Ok(Self::Description { name }),
                _ => Err(
                    "a board description is not combined with `input`, `source`, `path` or `file`"
                        .into(),
                ),
            };
        }
        match (text.input, text.source, text.path, text.file) {
            (Some(name), None, None, None) => Ok(Self::Input { name }),
            (Some(_), _, _, _) => {
                Err("a named input is not combined with `source`, `path` or `file`".into())
            }
            (None, Some(source), Some(path), None) => Ok(Self::InSource { source, path }),
            (None, None, None, Some(file)) => Ok(Self::Local { file }),
            (None, Some(_), None, None) => Err("`source` needs a `path` inside it".into()),
            _ => Err(
                "either `source` and `path`, `file`, `input`, `description` or `generated`".into(),
            ),
        }
    }
}

impl FileRef {
    pub(crate) fn resolve(&mut self, dir: &Path) {
        if let Self::Local { file } = self {
            file.resolve(dir);
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::InSource { source, .. } => Some(source),
            Self::Local { .. }
            | Self::Input { .. }
            | Self::Description { .. }
            | Self::Generated { .. } => None,
        }
    }
}

/// Whether `name` is a named input's name: lowercase letters, digits and
/// `-`, and not empty.
pub(crate) fn is_input_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Checks that `name`, used at `key` in `file`, is one of `sources`.
pub(crate) fn check_source<'a>(
    file: &Path,
    key: &str,
    name: &str,
    sources: impl Iterator<Item = &'a String>,
) -> Result<(), Error> {
    let known: Vec<String> = sources.cloned().collect();
    if known.iter().any(|known| known == name) {
        return Ok(());
    }
    let mut message = format!("no source named `{name}`");
    match crate::read::closest(name, &known) {
        Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
        None if known.is_empty() => message.push_str("; there are no sources"),
        None => message.push_str(&format!("; the sources are {}", known.join(", "))),
    }
    Err(Error::new(file, key, message))
}

/// Checks that the only `rust` source in `sources` is a component's
/// `toolchain` (`component` true): a Rust build's toolchain, which nothing
/// else takes.
pub(crate) fn check_rust_sources(
    file: &Path,
    sources: &BTreeMap<String, Source>,
    component: bool,
) -> Result<(), Error> {
    for (name, source) in sources {
        if matches!(source, Source::Rust { .. }) && !(component && name == "toolchain") {
            return Err(Error::new(
                file,
                format!("sources.{name}"),
                "a rust source is a Rust build's toolchain: a component's, named `toolchain`; nothing else takes one",
            ));
        }
    }
    Ok(())
}

/// Checks that `local`, written at `key` in `file`, is there.
pub(crate) fn check_exists(file: &Path, key: &str, local: &Path) -> Result<(), Error> {
    if local.exists() {
        Ok(())
    } else {
        Err(Error::new(
            file,
            key,
            format!("{} is not there", local.display()),
        ))
    }
}
