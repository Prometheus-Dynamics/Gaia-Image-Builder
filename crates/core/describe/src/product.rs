//! The product file: which components go on which boards, the OS each
//! image is built on, which recipe lays out each image, and the size budget.

use crate::common::{RelPath, SizeText, Source, check_source, resolve_sources};
use crate::{Board, Component, Error, read};
use gaia_lock::Want;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A product file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// The product file. Not written in it.
    #[serde(skip)]
    pub file: PathBuf,
    #[serde(skip_serializing)]
    version: i64,
    pub name: String,
    pub description: Option<String>,
    /// Sources the product itself names: an image's OS source. Locked as
    /// `<product>.<source>`.
    #[serde(default)]
    pub sources: BTreeMap<String, Source>,
    /// The component files it uses, by the name the images use.
    #[serde(default)]
    pub components: BTreeMap<String, RelPath>,
    pub images: Vec<ImageSpec>,
}

/// One image: a board, what goes on it, and how it is laid out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSpec {
    /// Names the image's outputs.
    pub name: String,
    /// The board file.
    pub board: RelPath,
    /// Names from `[components]`.
    #[serde(default)]
    pub components: Vec<String>,
    /// Names from `[components]` whose built trees fill the image's data
    /// partition: a recipe's `data` input.
    #[serde(default)]
    pub data: Vec<String>,
    /// Names from `[components]` whose installs make the image's initramfs
    /// tree, apart from its root filesystem: a recipe's `initramfs` input.
    /// A component is in one of the two trees, not both.
    #[serde(default)]
    pub initramfs: Vec<String>,
    /// The OS the image is built on. Unset: the recipe's inputs are only
    /// the components.
    pub os: Option<OsSpec>,
    /// The recipe that lays out the image.
    pub recipe: String,
    /// The most the image may be.
    pub budget: Option<SizeText>,
    /// The recipe's own settings. The recipe checks them when the graph is
    /// planned.
    #[serde(default)]
    pub settings: toml::Table,
}

/// The OS an image is built on: its provider, the provider's source and
/// the image's own config fragments. The board's kernel and bootloader are
/// added to it when the graph is planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OsSpec {
    pub provider: OsProvider,
    /// A source from the product's `[sources]`: the provider's own tree.
    pub source: String,
    /// The provider's config fragments, applied in order after what the
    /// board gives.
    #[serde(default)]
    pub fragments: Vec<RelPath>,
}

/// Who builds an image's OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsProvider {
    Buildroot,
}

/// A product and every file it names, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub product: Product,
    /// By board file.
    pub boards: BTreeMap<PathBuf, Board>,
    /// By the product's name for each.
    pub components: BTreeMap<String, Component>,
}

impl Product {
    /// Reads the product file at `path`, without the files it names.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let mut product: Self = read::read(path)?;
        product.file = path.to_path_buf();
        let dir = path.parent().unwrap_or(Path::new(""));
        resolve_sources(&mut product.sources, dir);
        for component in product.components.values_mut() {
            component.resolve(dir);
        }
        let known: Vec<String> = product.components.keys().cloned().collect();
        let mut names = std::collections::BTreeSet::new();
        for (index, image) in product.images.iter_mut().enumerate() {
            image.board.resolve(dir);
            if let Some(os) = &mut image.os {
                check_source(
                    path,
                    &format!("images[{index}].os.source"),
                    &os.source,
                    product.sources.keys(),
                )?;
                for fragment in &mut os.fragments {
                    fragment.resolve(dir);
                }
            }
            if !names.insert(image.name.clone()) {
                return Err(Error::new(
                    path,
                    format!("images[{index}].name"),
                    format!("`{}` names another image too", image.name),
                ));
            }
            if let Some(at) = image
                .initramfs
                .iter()
                .position(|name| image.components.contains(name))
            {
                return Err(Error::new(
                    path,
                    format!("images[{index}].initramfs[{at}]"),
                    format!(
                        "`{}` is in `components` too; a component is in the root filesystem or in the initramfs, not both",
                        image.initramfs[at]
                    ),
                ));
            }
            let lists = [
                ("components", &image.components),
                ("data", &image.data),
                ("initramfs", &image.initramfs),
            ];
            for (list, at, name) in lists.iter().flat_map(|(list, names)| {
                names.iter().enumerate().map(move |(at, n)| (*list, at, n))
            }) {
                if !known.contains(name) {
                    let mut message = format!("no component named `{name}` in [components]");
                    if let Some(near) = read::closest(name, &known) {
                        message.push_str(&format!("; did you mean `{near}`?"));
                    }
                    return Err(Error::new(
                        path,
                        format!("images[{index}].{list}[{at}]"),
                        message,
                    ));
                }
            }
        }
        if product.images.is_empty() {
            return Err(Error::new(
                path,
                "images",
                "a product has at least one image",
            ));
        }
        Ok(product)
    }

    /// Reads the product file at `path` and every board and component file
    /// it names.
    pub fn load(path: impl AsRef<Path>) -> Result<Loaded, Error> {
        let product = Self::read(path)?;
        let mut boards = BTreeMap::new();
        for image in &product.images {
            if !boards.contains_key(image.board.path()) {
                let board = Board::read(image.board.path())?;
                boards.insert(image.board.path().to_path_buf(), board);
            }
        }
        let mut components = BTreeMap::new();
        for (name, file) in &product.components {
            components.insert(name.clone(), Component::read(file.path())?);
        }
        Ok(Loaded {
            product,
            boards,
            components,
        })
    }
}

impl Product {
    /// What the lock must hold for the product's own sources, named
    /// `<product>.<source>`.
    pub fn wants(&self) -> Vec<(String, Want)> {
        self.sources
            .iter()
            .map(|(name, source)| (format!("{}.{name}", self.name), source.want()))
            .collect()
    }
}

impl Loaded {
    /// What the lock must hold: the product's, every board's and every
    /// component's sources.
    pub fn wants(&self) -> Vec<(String, Want)> {
        let mut wants: Vec<_> = self.product.wants();
        wants.extend(self.boards.values().flat_map(Board::wants));
        wants.extend(self.components.values().flat_map(Component::wants));
        wants.sort_by(|a, b| a.0.cmp(&b.0));
        wants.dedup_by(|a, b| a.0 == b.0);
        wants
    }
}
