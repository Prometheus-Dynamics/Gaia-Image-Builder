//! Checking one description file on its own: its format, and every name
//! and file it refers to. Nothing is fetched, locked or run.

use crate::{Board, Component, Error, Product, board};
use std::path::Path;

/// What a description file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Product,
    Board,
    Family,
    Component,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Board => "board",
            Self::Family => "family",
            Self::Component => "component",
        }
    }

    /// The kind named `text`, as `--kind` takes it.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Product, Self::Board, Self::Family, Self::Component]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// Top-level keys only a board or a family has.
const BOARD_KEYS: [&str; 8] = [
    "extends",
    "target",
    "kernel",
    "bootloader",
    "firmware",
    "partitions",
    "disk",
    "flash",
];

/// The kind of the file at `path`, from its top-level keys:
///
/// - `images`: a product.
/// - a key only boards have (`target`, `kernel`, `partitions`, ...): a
///   board, or a family when it has no `name` and no `extends`.
/// - anything else: a component.
pub fn kind_of(path: &Path) -> Result<Kind, Error> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::new(path, "", format!("cannot read: {error}")))?;
    let table: toml::Table =
        toml::from_str(&text).map_err(|error| Error::new(path, "", error.message().to_string()))?;
    Ok(if table.contains_key("images") {
        Kind::Product
    } else if BOARD_KEYS.iter().any(|key| table.contains_key(*key)) {
        if table.contains_key("name") || table.contains_key("extends") {
            Kind::Board
        } else {
            Kind::Family
        }
    } else {
        Kind::Component
    })
}

/// Checks the file at `path` as `kind`, or as the kind its keys give it.
///
/// - A product is read with every board and component file it names.
/// - A board is read with the family it extends.
/// - A family is checked alone: its keys, and the sources it names itself.
///
/// The first error is returned: the file, the key, and the line when known.
pub fn validate(path: &Path, kind: Option<Kind>) -> Result<Kind, Error> {
    let kind = match kind {
        Some(kind) => kind,
        None => kind_of(path)?,
    };
    match kind {
        Kind::Product => Product::load(path).map(|_| ())?,
        Kind::Board => Board::read(path).map(|_| ())?,
        Kind::Family => board::check_family(path)?,
        Kind::Component => Component::read(path).map(|_| ())?,
    }
    Ok(kind)
}
