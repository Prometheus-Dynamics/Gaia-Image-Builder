//! Gaia's descriptions: a board file, component files and a product file.
//!
//! Each is TOML with `version = 1` at the top. A key the format does not
//! have is an error, with the key meant when one is close. Every error
//! names the file and the key ([`Error`]), and the line when it is known.
//!
//! - **Board** ([`Board`]): what the hardware is and what it needs to boot.
//!   It may extend one family file (`extends = "../family/family.toml"`);
//!   nothing else is inherited or imported.
//! - **Component** ([`Component`]): one piece of software, how it is built
//!   and what it puts in the image.
//! - **Product** ([`Product`]): which components go on which boards, the
//!   OS each image is built on, the recipe for each image and its size
//!   budget.
//!
//! A path in a description is relative to the file it is written in. There
//! is no interpolation: a value is what is written.
//!
//! Sources are named per file and pinned by the lock; [`Loaded::wants`]
//! gives the lock check its list, named `<product, board or component>.<source>`.
//! A git source may name a `subdir`: a directory of its checkout to use as
//! the source's tree. The lock still pins the commit alone.

mod board;
mod common;
mod component;
mod error;
mod product;
mod read;
mod validate;

pub use board::{
    Board, Bootloader, Disk, Firmware, Flash, Kernel, Medium, PartitionNeed, Scheme,
    TrustedFirmware,
};
pub use common::{FileRef, ImagePlatform, Mode, RelPath, SizeText, Source, Subdir};
pub use component::{Build, Component, Group, Install, InstallFrom, Package, Service, User};
pub use error::Error;
pub use product::{ImageSpec, Loaded, OsProvider, OsSpec, Product};
pub use read::VERSION;
pub use validate::{Kind, kind_of, validate};
