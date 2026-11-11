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
//!   recipe for each image and its size budget.
//!
//! A path in a description is relative to the file it is written in. There
//! is no interpolation: a value is what is written.
//!
//! Sources are named per file and pinned by the lock; [`Loaded::wants`]
//! gives the lock check its list, named `<board or component>.<source>`.

mod board;
mod common;
mod component;
mod error;
mod product;
mod read;

pub use board::{Board, Bootloader, Firmware, Flash, Kernel, PartitionNeed, TrustedFirmware};
pub use common::{FileRef, Mode, RelPath, SizeText, Source};
pub use component::{Build, Component, Group, Install, InstallFrom, Service, User};
pub use error::Error;
pub use product::{ImageSpec, Loaded, Product};
pub use read::VERSION;
