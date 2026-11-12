//! The product file: which components go on which boards, which recipe
//! lays out each image, and the size budget.

use crate::common::{RelPath, SizeText};
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
    /// The recipe that lays out the image.
    pub recipe: String,
    /// The most the image may be.
    pub budget: Option<SizeText>,
    /// The recipe's own settings. The recipe checks them when the graph is
    /// planned.
    #[serde(default)]
    pub settings: toml::Table,
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
        for component in product.components.values_mut() {
            component.resolve(dir);
        }
        let known: Vec<String> = product.components.keys().cloned().collect();
        let mut names = std::collections::BTreeSet::new();
        for (index, image) in product.images.iter_mut().enumerate() {
            image.board.resolve(dir);
            if !names.insert(image.name.clone()) {
                return Err(Error::new(
                    path,
                    format!("images[{index}].name"),
                    format!("`{}` names another image too", image.name),
                ));
            }
            let lists = [("components", &image.components), ("data", &image.data)];
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

impl Loaded {
    /// What the lock must hold: every board's and component's sources.
    pub fn wants(&self) -> Vec<(String, Want)> {
        let mut wants: Vec<_> = self.boards.values().flat_map(Board::wants).collect();
        wants.extend(self.components.values().flat_map(Component::wants));
        wants.sort_by(|a, b| a.0.cmp(&b.0));
        wants.dedup_by(|a, b| a.0 == b.0);
        wants
    }
}
