//! Recipes as data.
//!
//! A recipe is a file: a named Daedalus node group (its stages, the
//! connections between them, the ports it exposes) plus parameters, each
//! with a type, an optional default and the stage ports it sets. Gaia's code
//! holds no recipe; a recipe is read like any other description and turned
//! into a [`GroupDoc`] with the values a product gives.
//!
//! ```toml
//! version = 1
//! name = "packed-rootfs"
//!
//! [stages]
//! rootfs = "gaia.os.none"
//! pack = "gaia.compress.zstd"
//!
//! [[connects]]
//! from = "rootfs.rootfs"
//! to = "pack.input"
//!
//! # A list of sources feeds a fan-in port in order: `merge.layers0`,
//! # `merge.layers1`, ...
//! # [[connects]]
//! # from = ["os.layer", "app.layer"]
//! # to = "merge.layers"
//!
//! [inputs]
//! store = ["rootfs.store", "pack.store"]
//!
//! [outputs]
//! packed = "pack.artifact"
//!
//! [parameters.level]
//! type = "int"
//! default = 3
//! sets = ["pack.level"]
//! ```
//!
//! Errors name the file, the recipe and the key or parameter.

mod error;
mod file;
mod param;

pub use error::Error;
pub use file::{Connect, Parameter, Recipe, Recipes, Sources};
pub use param::ParamType;

use daedalus::layers::graph::GroupDoc;

/// The version the recipe format is at.
pub const VERSION: i64 = 1;

impl Recipe {
    /// The group this recipe makes with `values` (from the file `from`,
    /// usually a product's image settings). A parameter not given takes its
    /// default; one with no default must be given.
    pub fn group(&self, values: &toml::Table, from: &std::path::Path) -> Result<GroupDoc, Error> {
        file::group(self, values, from)
    }
}
