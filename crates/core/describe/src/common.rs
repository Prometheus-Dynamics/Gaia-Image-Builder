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
/// `download`, `image` (with an optional `platform`) or `path`. The lock pins
/// the repository's commit; `subdir` is not in the lock. An `image` may name
/// its digest as `reference@sha256:<64 hex>`, which the lock then records
/// without asking the registry.
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
                return Err("say where it comes from: `git`, `download`, `image` or `path`".into());
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
    /// What the lock must hold for it.
    pub fn want(&self) -> Want {
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

#[cfg(test)]
mod tests {
    use super::{ImagePlatform, Source};

    fn source(text: &str) -> Result<Source, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn an_image_is_for_the_target_unless_it_says_host() {
        let image = |platform| Source::Image {
            reference: "r".into(),
            platform,
        };
        assert_eq!(
            source("image = \"r\"").expect("parses"),
            image(ImagePlatform::Target)
        );
        assert_eq!(
            source("image = \"r\"\nplatform = \"target\"").expect("parses"),
            image(ImagePlatform::Target)
        );
        assert_eq!(
            source("image = \"r\"\nplatform = \"host\"").expect("parses"),
            image(ImagePlatform::Host)
        );
        let other = source("image = \"r\"\nplatform = \"arm64\"").expect_err("refused");
        assert!(other.to_string().contains("platform"), "{other}");
        let git = source("git = \"u\"\nref = \"main\"\nplatform = \"host\"").expect_err("refused");
        assert!(
            git.to_string()
                .contains("only an image source has a `platform`"),
            "{git}"
        );
    }
}
