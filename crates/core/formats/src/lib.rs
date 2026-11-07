//! The on-media formats Gaia writes and Atlas reads: the format contract
//! between the image builder and the device. Gaia's assembly writes UBI
//! images and dm-verity trees with these types, and Atlas reads, flashes
//! and updates them with the same types, so the two cannot drift apart.
//!
//! # What is stable
//!
//! - [`ubi`]: the kernel's UBI on-media format (`drivers/mtd/ubi/ubi-media.h`):
//!   EC and VID headers, volume-table records, their CRCs, the flash
//!   geometry rules `ubinize` and the kernel share, the PEB reserves UBI
//!   keeps (`get_bad_peb_limit`), and the order Gaia lays an image out in.
//! - [`verity`]: the veritysetup (cryptsetup) version 1 superblock, the
//!   appended-tree layout, and Gaia's derivation of the default salt and
//!   UUID from the data's sha256 ([`verity::Derived`]).
//!
//! The kernel and cryptsetup define the first two; Gaia defines its layout
//! and its derivation, and changing either changes every image's bytes or
//! root hash.
//!
//! What a device does with these (Atlas's slot contract, which volume is
//! which slot) is Atlas's and is not here. Nothing here writes report files.
//!
//! # Versioning
//!
//! The crate versions with Gaia (`X.Y.Z-dev` on dev). Any change to a
//! format, a default or a derivation here needs a CHANGELOG line saying
//! what changes on the media, so Atlas can follow it.
//!
//! The crate depends on `std` and `sha2` only.

pub mod crc;
pub mod ubi;
pub mod verity;
